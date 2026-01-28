import groovy.json.JsonSlurper
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

    @Suppress("UNCHECKED_CAST")
    val json = JsonSlurper().parseText(metadata) as Map<String, Any?>
    @Suppress("UNCHECKED_CAST")
    val packages = json["packages"] as? List<Map<String, Any?>> ?: emptyList()
    val manifestPath = packages.firstOrNull { it["name"] == "rustls-platform-verifier-android" }
        ?.get("manifest_path") as? String
        ?: error("rustls-platform-verifier-android manifest path not found in cargo metadata")
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
                mavenPom()
                artifact()
            }
        }
    }
}

rootProject.name = "Connected"
include(":app")
