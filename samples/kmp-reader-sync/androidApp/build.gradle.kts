plugins {
    id("com.android.application")
    kotlin("android")
}

android {
    namespace = "com.kmosync.sample"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.kmosync.sample"
        minSdk = 23
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(project(":kmo-sync-kmp"))
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
}
