import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// The version is the desktop app's release number, `YYYY.M.D.count` (docs/releases.md): the one
// given with -Pmarquee.version (the release workflow gives the tag's), else `version` in
// android/version.properties, set by hand to the last release's. It is the versionName. The
// versionCode, which Android compares and which must grow with every release the phone installs,
// follows from it: the date as YYMMDD, then the day's count in two digits (2026.11.20.1 is
// 26112001), so a later release always has a higher one.
val versionProps = Properties().apply { rootProject.file("version.properties").inputStream().use { load(it) } }
val appVersion: String = providers.gradleProperty("marquee.version").orNull?.trim()?.takeIf(String::isNotEmpty)
    ?: versionProps.getProperty("version")?.trim()
    ?: throw GradleException("No version= in version.properties")
val appVersionCode: Int = run {
    val n = appVersion.split(".").map { it.toIntOrNull() }
    val ok = n.size == 4 && n.all { it != null } && n[0]!! in 2000..2099 && n[1]!! in 1..12 && n[2]!! in 1..31 && n[3]!! in 1..99
    if (!ok) throw GradleException("The version $appVersion is not a release number (YYYY.M.D.count, a count of 1 to 99)")
    (n[0]!! % 100) * 1_000_000 + n[1]!! * 10_000 + n[2]!! * 100 + n[3]!!
}

// The APKs are app/build/outputs/apk/<variant>/marquee-player-<version>-<variant>.apk.
base { archivesName.set("marquee-player-$appVersion") }

// The release key never lives in the repo. Either all four MARQUEE_KEYSTORE_FILE,
// MARQUEE_KEYSTORE_PASSWORD, MARQUEE_KEY_ALIAS, MARQUEE_KEY_PASSWORD are set (CI, another machine),
// or ~/.marquee-android/keystore.properties and release.jks are there (a developer's machine; build.ps1
// -Release creates them on first use). The environment wins. Without either, the release is
// left unsigned, and debug builds and tests never need it. None of the values is ever printed.
class ReleaseKey(val storeFile: File, val storePassword: String, val keyAlias: String, val keyPassword: String)

val releaseKey: ReleaseKey? = run {
    val env = listOf("MARQUEE_KEYSTORE_FILE", "MARQUEE_KEYSTORE_PASSWORD", "MARQUEE_KEY_ALIAS", "MARQUEE_KEY_PASSWORD")
        .map { providers.environmentVariable(it).orNull?.takeIf(String::isNotEmpty) }
    if (env.all { it != null }) {
        return@run ReleaseKey(file(env[0]!!), env[1]!!, env[2]!!, env[3]!!)
    }
    if (env.any { it != null }) {
        throw GradleException("Set all four of MARQUEE_KEYSTORE_FILE, MARQUEE_KEYSTORE_PASSWORD, MARQUEE_KEY_ALIAS and MARQUEE_KEY_PASSWORD, or none")
    }
    val dir = File(System.getProperty("user.home"), ".marquee-android")
    val propsFile = File(dir, "keystore.properties").takeIf { it.isFile } ?: return@run null
    val p = Properties().apply { propsFile.inputStream().use { load(it) } }
    ReleaseKey(
        File(dir, p.getProperty("storeFile", "release.jks")),
        p.getProperty("storePassword"), p.getProperty("keyAlias"), p.getProperty("keyPassword"),
    )
}

android {
    namespace = "com.offbeat.marquee.player"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.offbeat.marquee.player"
        minSdk = libs.versions.minSdk.get().toInt()
        targetSdk = 35
        versionCode = appVersionCode
        versionName = appVersion
        // Where the app's source code is published, shown in Settings > About when set.
        // Empty hides the "Source code" row.
        buildConfigField("String", "SOURCE_URL", "\"https://github.com/OffbeatDX/MarqueePlayer\"")
    }

    signingConfigs {
        if (releaseKey != null) {
            create("release") {
                storeFile = releaseKey.storeFile
                storePassword = releaseKey.storePassword
                keyAlias = releaseKey.keyAlias
                keyPassword = releaseKey.keyPassword
                enableV1Signing = false
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
            // Debug builds talk to the server read-only (android/docs/DESIGN.md, "Testing rules").
            buildConfigField("boolean", "READ_ONLY", "true")
            // No LRCLIB in test builds: lookups are off unless a local stand-in is pushed (loopback only).
            buildConfigField("String", "LRCLIB_URL", "\"\"")
        }
        release {
            buildConfigField("boolean", "READ_ONLY", "false")
            buildConfigField("String", "LRCLIB_URL", "\"https://lrclib.net\"")
            // Signed with the release key above; unsigned when there is none.
            signingConfig = signingConfigs.findByName("release")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
        // For measuring scrolling and start-up: the release build (R8, not debuggable, profileable),
        // signed with the debug key, installed beside the others (.bench), and read-only like
        // debug, as it is a test build too. It reads the pushed test config like debug does.
        create("benchmark") {
            initWith(getByName("release"))
            applicationIdSuffix = ".bench"
            versionNameSuffix = "-bench"
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += listOf("release")
            buildConfigField("boolean", "READ_ONLY", "true")
            buildConfigField("String", "LRCLIB_URL", "\"\"")
        }
    }
    sourceSets {
        // The debug-only test config loader (the release stub never reads it).
        getByName("benchmark").kotlin.srcDir("src/debug/kotlin")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    lint {
        // The issues Lint found when CI was added (mostly Media3's @UnstableApi, used on purpose)
        // are recorded here, so only new ones fail. Regenerate: ./gradlew :app:updateLintBaseline
        baseline = file("lint-baseline.xml")
    }
    packaging {
        resources {
            excludes += setOf("META-INF/{AL2.0,LGPL2.1}", "META-INF/LICENSE*", "META-INF/NOTICE*")
        }
    }
}

dependencies {
    implementation(project(":core"))

    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.ktx)

    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons)
    implementation(libs.androidx.navigation.compose)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)

    implementation(libs.androidx.media3.exoplayer)
    implementation(libs.androidx.media3.session)
    implementation(libs.androidx.media3.datasource.okhttp)
    implementation(libs.androidx.work.runtime)
    implementation(libs.androidx.glance.appwidget)

    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.kotlinx.coroutines.test)
}
