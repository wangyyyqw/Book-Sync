plugins {
    kotlin("multiplatform")
    id("com.android.application")
}

kotlin {
    jvm()
    androidTarget()
    jvmToolchain(17)

    sourceSets {
        commonMain.dependencies {
            implementation(project(":kmo-sync-kmp"))
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
            implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
        }
        jvmMain.dependencies {
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
        }
        androidMain.dependencies {
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
    }
}

// KMP 模块不能应用 `application` 插件；用 JavaExec 任务跑 JVM 命令行版：
//   gradle :samples:guanzhi-sync-demo:runDemo
val runDemo by tasks.registering(JavaExec::class) {
    group = "application"
    description = "Runs the JVM command-line two-device sync demo."
    mainClass.set("guanzhi.syncdemo.JvmDemoMainKt")
    dependsOn(tasks.named("jvmMainClasses"))
    val jvmMain = kotlin.targets.getByName("jvm").compilations.getByName("main")
    classpath = files(
        jvmMain.output.allOutputs,
        jvmMain.runtimeDependencyFiles,
    )
}

android {
    namespace = "guanzhi.syncdemo"
    compileSdk = 34

    defaultConfig {
        applicationId = "guanzhi.syncdemo"
        minSdk = 23
        targetSdk = 34
        versionCode = 1
        versionName = "1.0.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
