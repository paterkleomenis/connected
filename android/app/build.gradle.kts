plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
    }
}

android {
    namespace = "com.connected.app"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.connected.app"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        ndk {
            // Target architectures for the Rust library
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
        isCoreLibraryDesugaringEnabled = true
    }

    kotlinOptions {
        jvmTarget = "21"
    }

    // Configure where to find the native libraries (.so files)
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

dependencies {
    // Core library desugaring for Java 21 APIs on older Android
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.2")

    // AndroidX Core
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("com.google.android.material:material:1.11.0")

    // Coroutines for async operations
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.7.3")

    // JNA for UniFFI bindings
    implementation("net.java.dev.jna:jna:5.14.0@aar")

    // Testing
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.5")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.1")
}

// Task to build Rust library for Android
tasks.register<Exec>("buildRustDebug") {
    workingDir = file("${project.rootDir}/../ffi")
    commandLine("cargo", "ndk",
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
    commandLine("cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-t", "x86_64",
        "-t", "x86",
        "-o", "${project.projectDir}/src/main/jniLibs",
        "build", "--release"
    )
}

// Generate UniFFI Kotlin bindings (using bundled uniffi-bindgen)
tasks.register<Exec>("generateBindings") {
    workingDir = file("${project.rootDir}/..")
    commandLine("cargo", "run", "--release",
        "-p", "connected-ffi",
        "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", "target/release/libconnected_ffi.so",
        "--language", "kotlin",
        "--out-dir", "${project.projectDir}/src/main/kotlin",
        "--no-format"
    )
    dependsOn("buildRustRelease")
}
