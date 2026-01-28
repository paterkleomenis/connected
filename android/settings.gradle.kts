import java.io.File

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "1.0.0"
}

fun rustlsPlatformVerifierRepo(settingsDir: File): File {
    val metadata = providers.exec {
        workingDir = settingsDir.parentFile
        commandLine(
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            "aarch64-linux-android",
            "--manifest-path",
            File(settingsDir.parentFile, "Cargo.toml").path
        )
    }.standardOutput.asText.get()

    val regex = Regex(
        "\"name\"\\s*:\\s*\"rustls-platform-verifier-android\"[\\s\\S]*?\"manifest_path\"\\s*:\\s*\"([^\"]+)\""
    )
    val match = regex.find(metadata)
        ?: error("rustls-platform-verifier-android manifest path not found in cargo metadata")
    val manifestPath = match.groupValues[1].replace("\\\\", "\\")
    return File(manifestPath).parentFile.resolve("maven")
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        maven {
            url = uri(rustlsPlatformVerifierRepo(settingsDir))
            metadataSources {
                artifact()
            }
        }
    }
}

rootProject.name = "Connected"
include(":app")
