import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.kotlin.serialization)
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    api(libs.kotlinx.coroutines.core)
    api(libs.kotlinx.serialization.json)
    api(libs.okhttp)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)
}

tasks.test {
    // The live tests read ~/.marquee-android/test-server.properties when it exists; they report
    // as skipped otherwise. Output is kept quiet so nothing from the server lands in logs.
    testLogging { events("passed", "skipped", "failed") }
    // -Pmarquee.testUserHome=<dir> runs the tests with another home, as on a machine without the
    // test server (the live tests then skip).
    providers.gradleProperty("marquee.testUserHome").orNull?.let { systemProperty("user.home", it) }
}

// :core is a plain JVM module, so Lint's NewApi check never sees it: scan its classes for
// java.*/android.* calls newer than the app's minSdk (an API 33 call once crashed Android 12 at
// start). Part of `check`; skipped with a warning when the SDK's api-versions.xml is missing.
val apiScan by tasks.registering(ApiScanTask::class) {
    classes.from(sourceSets.main.map { it.output.classesDirs })
    minSdk.set(libs.versions.minSdk.map { it.toInt() })
    ApiScanTask.findApiVersionsXml(rootDir) { providers.environmentVariable(it).orNull }
        ?.let { apiVersions.set(it) }
    javaHome.set(providers.systemProperty("java.home"))
    report.set(layout.buildDirectory.file("reports/apiScan/apiScan.txt"))
}
tasks.check { dependsOn(apiScan) }
