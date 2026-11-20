import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.Properties
import javax.inject.Inject
import javax.xml.stream.XMLInputFactory
import javax.xml.stream.XMLStreamConstants

/**
 * Fails when compiled classes call a java.* / javax.* / android.* API newer than the app's minSdk.
 *
 * Android Lint's NewApi check never sees :core (a plain JVM module), and the JDK has methods that
 * Android only gained later: `URLEncoder.encode(String, Charset)` is API 33, and it crashed the app
 * on Android 12. This reads the SDK's api-versions.xml and every call and field reference in the
 * classes (through the JDK's `javap -c`), and lists those whose API level is above minSdk.
 *
 * Members the XML lists only on a superclass (inherited ones) are not resolved and are skipped:
 * in practice they are not newer than the class they are called on.
 *
 * Without the SDK's data file (platforms/android-N/data/api-versions.xml) the scan is skipped with
 * a warning, so a machine without the SDK can still run the JVM tests.
 */
abstract class ApiScanTask : DefaultTask() {
    /** Directories of compiled classes to scan. */
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val classes: ConfigurableFileCollection

    @get:Input
    abstract val minSdk: Property<Int>

    /** The SDK's api-versions.xml; absent when the SDK or its data file is missing. */
    @get:InputFile
    @get:Optional
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val apiVersions: RegularFileProperty

    /** The JDK the build runs on: javap comes from it. */
    @get:Input
    abstract val javaHome: Property<String>

    @get:OutputFile
    abstract val report: RegularFileProperty

    @get:Inject
    abstract val execOps: ExecOperations

    init {
        group = "verification"
        description = "Fails when the classes call a java.*/android.* API newer than minSdk (Lint can't see a JVM module)."
        // A skipped scan is never up to date, so the warning shows on every run.
        outputs.upToDateWhen { apiVersions.isPresent }
    }

    @TaskAction
    fun scan() {
        val min = minSdk.get()
        val out = report.get().asFile
        if (!apiVersions.isPresent) {
            val msg = "apiScan skipped: no platforms/android-*/data/api-versions.xml in the Android SDK " +
                "(set sdk.dir in local.properties or ANDROID_HOME); :core was not checked against minSdk $min"
            logger.warn("WARNING: $msg")
            out.writeText("$msg\n")
            return
        }
        val apis = readApis(apiVersions.get().asFile)
        val javap = File(javaHome.get(), "bin/" + if (isWindows()) "javap.exe" else "javap")
        if (!javap.isFile) throw GradleException("apiScan: no javap in ${javaHome.get()} (Gradle must run on a JDK, not a JRE)")

        val files = classes.asFileTree.matching { include("**/*.class") }.files.sortedBy { it.path }
        val ref = Regex("""// (Method|InterfaceMethod|Field) ([\w/$]+)\.("?<?[\w$]+>?"?):(\S+)""")
        val header = Regex("""^(public |final |abstract )*(class|interface) """)
        val hits = sortedMapOf<String, MutableSet<String>>()
        for (batch in files.chunked(40)) {
            val stdout = ByteArrayOutputStream()
            execOps.exec {
                commandLine(listOf(javap.path, "-c", "-p") + batch.map { it.path })
                standardOutput = stdout
            }
            var where: String? = null
            for (line in stdout.toString(Charsets.ISO_8859_1).lineSequence()) {
                if (header.containsMatchIn(line)) where = line.substringBefore('{').trim()
                for (m in ref.findAll(line)) {
                    val (kind, owner, name, desc) = m.destructured
                    if (!(owner.startsWith("java/") || owner.startsWith("javax/") || owner.startsWith("android/"))) continue
                    val entry = apis[owner]
                    if (entry == null) {
                        hits.getOrPut("$owner: class not on Android") { sortedSetOf() }.addIfNotNull(where)
                        continue
                    }
                    val key = name.trim('"') + if (kind == "Field") "" else desc
                    val since = entry.members[key] ?: if (kind == "Field") entry.since else null
                    if (since != null && since > min) {
                        hits.getOrPut("$owner.$key: API $since") { sortedSetOf() }.addIfNotNull(where)
                    }
                }
            }
        }

        val text = buildString {
            for ((api, users) in hits) {
                appendLine(api)
                users.take(3).forEach { appendLine("    in $it") }
            }
            if (hits.isEmpty()) appendLine("apiScan: nothing above minSdk $min in ${files.size} classes")
            else appendLine("${hits.size} API(s) above minSdk $min")
        }
        out.writeText(text)
        if (hits.isNotEmpty()) {
            throw GradleException("These classes call APIs newer than minSdk $min (Android would crash there):\n$text")
        }
        logger.lifecycle(text.trim())
    }

    private class Api(val since: Int, val members: Map<String, Int>)

    private fun readApis(xml: File): Map<String, Api> {
        val apis = HashMap<String, Api>(8192)
        val factory = XMLInputFactory.newInstance().apply {
            setProperty(XMLInputFactory.SUPPORT_DTD, false)
            setProperty(XMLInputFactory.IS_SUPPORTING_EXTERNAL_ENTITIES, false)
        }
        xml.inputStream().buffered().use { input ->
            val r = factory.createXMLStreamReader(input)
            var cls: String? = null
            var clsSince = 1
            var members = HashMap<String, Int>()
            var depth = 0
            while (r.hasNext()) {
                when (r.next()) {
                    XMLStreamConstants.START_ELEMENT -> {
                        depth++
                        val since = r.getAttributeValue(null, "since")?.toIntOrNull()
                        if (depth == 2 && r.localName == "class") {
                            cls = r.getAttributeValue(null, "name")
                            clsSince = since ?: 1
                            members = HashMap()
                        } else if (depth == 3 && cls != null && (r.localName == "method" || r.localName == "field")) {
                            members[r.getAttributeValue(null, "name")] = since ?: clsSince
                        }
                    }
                    XMLStreamConstants.END_ELEMENT -> {
                        if (depth == 2 && cls != null) {
                            apis[cls] = Api(clsSince, members)
                            cls = null
                        }
                        depth--
                    }
                }
            }
            r.close()
        }
        return apis
    }

    private fun MutableSet<String>.addIfNotNull(s: String?) {
        if (s != null) add(s)
    }

    private fun isWindows() = System.getProperty("os.name").startsWith("Windows", ignoreCase = true)

    companion object {
        /**
         * The newest platforms/android-N/data/api-versions.xml of the Android SDK, found as the
         * Android Gradle plugin finds the SDK: sdk.dir in local.properties, then ANDROID_HOME, then
         * ANDROID_SDK_ROOT. Null when there is none.
         */
        fun findApiVersionsXml(rootDir: File, env: (String) -> String?): File? {
            val local = File(rootDir, "local.properties").takeIf { it.isFile }?.let { f ->
                Properties().apply { f.inputStream().use { load(it) } }.getProperty("sdk.dir")
            }
            val sdk = listOfNotNull(local, env("ANDROID_HOME"), env("ANDROID_SDK_ROOT"))
                .map(::File).firstOrNull { it.isDirectory } ?: return null
            return File(sdk, "platforms").listFiles().orEmpty()
                .filter { it.isDirectory }
                .sortedByDescending { it.name.substringAfterLast('-').toIntOrNull() ?: -1 }
                .map { File(it, "data/api-versions.xml") }
                .firstOrNull { it.isFile }
        }
    }
}
