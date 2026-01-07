plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
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
        vectorDrawables {
            useSupportLibrary = true
        }

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

    buildFeatures {
        compose = true
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
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
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.7.0")
    implementation("androidx.activity:activity-compose:1.8.2")

    // Compose
    implementation(platform("androidx.compose:compose-bom:2023.08.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")

    // Material Components (Required for Theme.MaterialComponents in themes.xml)
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("com.google.android.material:material:1.11.0")

    // JNA for UniFFI bindings
    implementation("net.java.dev.jna:jna:5.14.0@aar")

    // Testing
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.5")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.1")
    androidTestImplementation(platform("androidx.compose:compose-bom:2023.08.00"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
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
// We use the library built for x86_64 (emulator) or arm64 as a reference for generation.
// It doesn't matter which architecture, as long as the API is the same.
tasks.register<Exec>("generateBindings") {
    workingDir = file("${project.rootDir}/..")
    // Use the x86_64 debug lib for generation speed/convenience during debug builds
    commandLine("cargo", "run", "--release",
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

afterEvaluate {
    tasks.named("preDebugBuild").configure {
        dependsOn("generateBindings")
    }
    tasks.named("preReleaseBuild").configure {
        dependsOn("buildRustRelease")
    }
}
