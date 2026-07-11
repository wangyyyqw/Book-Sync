import org.gradle.api.tasks.Copy
import org.gradle.api.tasks.Exec
import java.util.Locale

plugins {
    kotlin("multiplatform")
    id("com.android.library")
}

val rustProjectDir = rootProject.layout.projectDirectory.dir("kmo_sync")
val rustSourceInputs = fileTree(rustProjectDir) {
    include("Cargo.toml", "Cargo.lock", "build.rs", "src/**", "include/**")
}

val hostOs = System.getProperty("os.name").lowercase(Locale.ROOT).let {
    when {
        it.contains("mac") -> "macos"
        it.contains("win") -> "windows"
        else -> "linux"
    }
}
val hostArch = System.getProperty("os.arch").lowercase(Locale.ROOT).let {
    when (it) {
        "aarch64", "arm64" -> "aarch64"
        else -> "x86_64"
    }
}
val hostNativeLibrary = when (hostOs) {
    "macos" -> "libkmo_sync.dylib"
    "windows" -> "kmo_sync.dll"
    else -> "libkmo_sync.so"
}
val hostNativeOutput = rustProjectDir.file("target/release/$hostNativeLibrary")
val generatedJvmResources = layout.buildDirectory.dir("generated/jvmNativeResources")

val buildRustJvmHost by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds the Rust native library for this JVM host."
    workingDir(rustProjectDir)
    commandLine("cargo", "build", "--release")
    inputs.files(rustSourceInputs)
    outputs.file(hostNativeOutput)
}

val packageRustJvmHost by tasks.registering(Copy::class) {
    group = "rust"
    description = "Packages the host Rust library into the JVM artifact."
    dependsOn(buildRustJvmHost)
    from(hostNativeOutput)
    into(generatedJvmResources.map { it.dir("native/$hostOs-$hostArch") })
}

val buildRustAndroid by tasks.registering(Exec::class) {
    group = "rust"
    description = "Builds all Android Rust libraries required by the AAR."
    workingDir(rootProject.projectDir)
    commandLine("sh", rustProjectDir.file("scripts/build_android.sh").asFile.absolutePath)
    inputs.files(rustSourceInputs, rustProjectDir.file("scripts/build_android.sh"))
    outputs.files(
        file("src/androidMain/jniLibs/arm64-v8a/libkmo_sync.so"),
        file("src/androidMain/jniLibs/armeabi-v7a/libkmo_sync.so"),
        file("src/androidMain/jniLibs/x86_64/libkmo_sync.so"),
    )
}

val iosRustTargets = mapOf(
    "IosArm64" to "aarch64-apple-ios",
    "IosSimulatorArm64" to "aarch64-apple-ios-sim",
)
val installRustIosTargets = iosRustTargets.mapValues { (_, rustTarget) ->
    tasks.register<Exec>("installRust${rustTarget.replace("-", "_")}") {
        group = "rust"
        description = "Installs the Rust standard library for $rustTarget."
        commandLine("rustup", "target", "add", rustTarget)
    }
}
val buildRustIosTasks = iosRustTargets.mapValues { (taskSuffix, rustTarget) ->
    tasks.register<Exec>("buildRust${rustTarget.replace("-", "_")}") {
        group = "rust"
        description = "Builds the Rust library for $rustTarget."
        dependsOn(installRustIosTargets.getValue(taskSuffix))
        workingDir(rustProjectDir)
        commandLine("cargo", "build", "--release", "--target", rustTarget)
        inputs.files(rustSourceInputs)
        outputs.file(rustProjectDir.file("target/$rustTarget/release/libkmo_sync.a"))
    }
}

kotlin {
    jvmToolchain(17)

    jvm()
    androidTarget()
    iosArm64()
    iosSimulatorArm64()

    targets.withType<org.jetbrains.kotlin.gradle.plugin.mpp.KotlinNativeTarget>().configureEach {
        val rustTarget = if (konanTarget.name == "ios_arm64") {
            "aarch64-apple-ios"
        } else {
            "aarch64-apple-ios-sim"
        }
        compilations.getByName("main").cinterops.create("kmoSync") {
            defFile(project.file("src/nativeInterop/cinterop/kmo_sync.def"))
            compilerOpts("-I${project.rootDir}/kmo_sync/include")
        }
        binaries.all {
            linkerOpts("-L${project.rootDir}/kmo_sync/target/$rustTarget/release", "-lkmo_sync")
        }
        binaries.framework {
            baseName = "KmoSyncKmp"
        }
    }

    sourceSets {
        commonMain.dependencies {
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
        jvmMain {
            resources.srcDir(generatedJvmResources)
            dependencies {
                implementation("net.java.dev.jna:jna:5.14.0")
            }
        }
    }
}

android {
    namespace = "com.kmosync"
    compileSdk = 34
    defaultConfig {
        minSdk = 23
        consumerProguardFiles("consumer-rules.pro")
    }
}

tasks.named("preBuild").configure {
    dependsOn(buildRustAndroid)
}
tasks.named("jvmProcessResources").configure {
    dependsOn(packageRustJvmHost)
}
tasks.configureEach {
    iosRustTargets.forEach { (taskSuffix, rustTarget) ->
        if (name.contains(taskSuffix, ignoreCase = true) &&
            (name.startsWith("link") || name.startsWith("cinterop"))
        ) {
            dependsOn(buildRustIosTasks.getValue(taskSuffix))
            if (name.startsWith("link")) {
                inputs.file(rustProjectDir.file("target/$rustTarget/release/libkmo_sync.a"))
            }
        }
    }
}
