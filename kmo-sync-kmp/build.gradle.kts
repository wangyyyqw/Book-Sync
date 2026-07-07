plugins {
    kotlin("multiplatform")
    id("com.android.library")
}

kotlin {
    jvmToolchain(17)

    jvm()
    androidTarget()

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
