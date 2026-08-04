pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "Book Sync"

include(":kmo-sync-kmp")
include(":samples:kmp-reader-sync:androidApp")
include(":samples:guanzhi-sync-demo")
