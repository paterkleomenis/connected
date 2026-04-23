import com.android.build.api.dsl.ApplicationExtension
import org.gradle.api.JavaVersion
import java.util.Properties
import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Helper to find cargo
val cargoExecutable: String = run {
    val os = System.getProperty("os.name").lowercase()
    val isWindows = os.contains("win")
    val cargoName = if (isWindows) "cargo.exe" else "cargo"
    val homeCargo = File(System.getProperty("user.home"), ".cargo/bin/$cargoName")
    if (homeCargo.exists()) homeCargo.absolutePath else cargoName
}

// Helper to find SDK and NDK
val sdkDir = project.rootProject.file("local.properties").let { localProps ->
    if (localProps.exists()) {
        val p = Properties()
        localProps.inputStream().use { p.load(it) }
        p.getProperty("sdk.dir")?.let { File(it) }
    } else {
        null
    }
} ?: System.getenv("ANDROID_HOME")?.let { File(it) }
  ?: throw GradleException("Android SDK not found. Please set local.properties or ANDROID_HOME environment variable.")

val ndkDir = File(sdkDir, "ndk").listFiles()
    ?.filter { it.isDirectory }?.maxByOrNull { it.name }
    ?: throw GradleException("NDK not found in ${File(sdkDir, "ndk")}")

val latestNdkVersion: String = ndkDir.name

kotlin {
    compilerOptions {
        freeCompilerArgs.add("-opt-in=androidx.compose.material3.ExperimentalMaterial3Api")
    }
    jvmToolchain(JavaVersion.current().majorVersion.toInt())
}

configure<ApplicationExtension> {
    namespace = "com.connected.app.sync"
    compileSdk = 37
    ndkVersion = latestNdkVersion

    defaultConfig {
        applicationId = "com.connected.app.sync"
        minSdk = 26
        targetSdk = 37
        versionCode = 51
        versionName = "2.9.6"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        vectorDrawables {
            useSupportLibrary = true
        }

        ndk {
            // Target architectures for the Rust library
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    // Load properties from .env file for Android Studio compatibility
    val envFile = project.rootProject.file(".env")
    val env = Properties()
    if (envFile.exists()) {
        envFile.inputStream().use { env.load(it) }
    }

    signingConfigs {
        create("release") {
            val keystorePath = env.getProperty("ANDROID_KEYSTORE_PATH")
                ?: System.getenv("ANDROID_KEYSTORE_PATH")
                ?: "release.keystore"
            storeFile = file(keystorePath)
            storePassword = env.getProperty("ANDROID_KEYSTORE_PASSWORD")
                ?: System.getenv("ANDROID_KEYSTORE_PASSWORD")
            keyAlias = env.getProperty("ANDROID_KEY_ALIAS")
                ?: System.getenv("ANDROID_KEY_ALIAS")
            keyPassword = env.getProperty("ANDROID_KEY_PASSWORD")
                ?: System.getenv("ANDROID_KEY_PASSWORD")
        }
    }
    buildTypes {
        getByName("debug") {
            // Allow debug/dev install side-by-side with production app.
            applicationIdSuffix = ".dev"
            versionNameSuffix = "-dev"
        }

        release {
            isMinifyEnabled = true
            // Use release signing if credentials are found in .env or environment
            val hasSigning = env.getProperty("ANDROID_KEYSTORE_PASSWORD") != null
                || System.getenv("ANDROID_KEYSTORE_PASSWORD") != null

            signingConfig = if (hasSigning) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        isCoreLibraryDesugaringEnabled = true
    }

    buildFeatures {
        compose = true
    }

    lint {
        disable += "MutableCollectionMutableState"
        disable += "AutoboxingStateCreation"
        baseline = file("lint-baseline.xml")
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }

    // Configure where to find the native libraries (.so files)
    sourceSets {
        getByName("main") {
            jniLibs.directories.add("src/main/jniLibs")
        }
    }
}

dependencies {
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    // Core library desugaring for Java 21 APIs on older Android
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")

    // AndroidX Core
    implementation("androidx.core:core-ktx:1.18.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.10.0")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.media:media:1.7.1")
    implementation("androidx.documentfile:documentfile:1.1.0")

    // Compose
    implementation(platform("androidx.compose:compose-bom:2026.04.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Material Components (Required for Theme.MaterialComponents in themes.xml)
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.13.0")

    // JNA for UniFFI bindings
    implementation("net.java.dev.jna:jna:5.18.1@aar")

    // Android verifier component for rustls-platform-verifier
    implementation("rustls:rustls-platform-verifier:0.1.1")

    // Testing
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation(platform("androidx.compose:compose-bom:2026.04.01"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}

// Task to build Rust library for Android
tasks.register<Exec>("buildRustDebug") {
    workingDir = file("${project.rootDir}/../ffi")
    environment("ANDROID_NDK_HOME", ndkDir.absolutePath)
    commandLine(cargoExecutable, "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-t", "x86_64",
        "-t", "x86",
        "-o", "${project.projectDir}/src/main/jniLibs",
        "build"
    )
}

tasks.register<Exec>("buildRustRelease") {
    workingDir = file("${project.rootDir}/../ffi")
    environment("ANDROID_NDK_HOME", ndkDir.absolutePath)
    commandLine(cargoExecutable, "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-t", "x86_64",
        "-t", "x86",
        "-o", "${project.projectDir}/src/main/jniLibs",
        "build", "--release"
    )
}

// Generate UniFFI Kotlin bindings (using bundled uniffi-bindgen)
// We use the library built for x86_64 (emulator) or arm64 as a reference for generation.
// It doesn't matter which architecture, as long as the API is the same.
tasks.register<Exec>("generateBindings") {
    workingDir = file("${project.rootDir}/..")
    // Use the x86_64 debug lib for generation speed/convenience during debug builds
    commandLine(cargoExecutable, "run", "--release",
        "-p", "connected-ffi",
        "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", "target/aarch64-linux-android/debug/libconnected_ffi.so",
        "--language", "kotlin",
        "--out-dir", "${project.projectDir}/src/main/kotlin",
        "--no-format"
    )
    // Ensure the library exists before generating bindings.
    // We depend on buildRustDebug because we point to the debug .so
    dependsOn("buildRustDebug")
}

tasks.register<Exec>("generateBindingsRelease") {
    workingDir = file("${project.rootDir}/..")
    // Use the aarch64 release lib for generation
    commandLine(cargoExecutable, "run", "--release",
        "-p", "connected-ffi",
        "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", "target/aarch64-linux-android/release/libconnected_ffi.so",
        "--language", "kotlin",
        "--out-dir", "${project.projectDir}/src/main/kotlin",
        "--no-format"
    )
    dependsOn("buildRustRelease")
}

afterEvaluate {
    tasks.matching { it.name == "preDebugBuild" }.configureEach {
        dependsOn("generateBindings")
    }
    tasks.matching { it.name == "preReleaseBuild" }.configureEach {
        dependsOn("generateBindingsRelease")
    }
}
