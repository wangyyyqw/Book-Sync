plugins {
    kotlin("multiplatform")
    id("com.android.library")
}

kotlin {
    jvmToolchain(17)

    jvm()
    androidTarget()
    iosArm64()
    iosSimulatorArm64()

    targets.withType<org.jetbrains.kotlin.gradle.plugin.mpp.KotlinNativeTarget>().configureEach {
        compilations.getByName("main").cinterops.create("kmoSync") {
            defFile(project.file("src/nativeInterop/cinterop/kmo_sync.def"))
            compilerOpts("-I${project.rootDir}/kmo_sync/include")
        }
        binaries.framework {
            baseName = "KmoSyncKmp"
            val rustTarget = if (konanTarget.name == "ios_arm64") {
                "aarch64-apple-ios"
            } else {
                "aarch64-apple-ios-sim"
            }
            linkerOpts("-L${project.rootDir}/kmo_sync/target/$rustTarget/release", "-lkmo_sync")
        }
    }

    sourceSets {
        commonMain.dependencies {
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
        jvmMain.dependencies {
            implementation("net.java.dev.jna:jna:5.14.0")
        }
    }
}

android {
    namespace = "com.kmosync"
    compileSdk = 34
    defaultConfig {
        minSdk = 23
    }
}
