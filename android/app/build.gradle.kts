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
        versionCode = 30206
        versionName = "3.2.6"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        vectorDrawables {
            useSupportLibrary = true
        }

        resValue("string", "app_name", "Connected")

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

    flavorDimensions += "distribution"
    productFlavors {
        create("play") {
            dimension = "distribution"
        }
        create("standalone") {
            dimension = "distribution"
        }
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
            resValue("string", "app_name", "Connected Dev")

            // Use release signing for debug builds if credentials are found,
            // so developers can sync signatures across machines.
            val hasSigning = env.getProperty("ANDROID_KEYSTORE_PASSWORD") != null
                || System.getenv("ANDROID_KEYSTORE_PASSWORD") != null

            if (hasSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
        }

        release {
            isMinifyEnabled = true
            isShrinkResources = true
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
        resValues = true
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
    implementation("androidx.core:core-ktx:1.19.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.11.0")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.documentfile:documentfile:1.1.0")

    // Media3
    val media3Version = "1.10.1"
    implementation("androidx.media3:media3-session:$media3Version")
    implementation("androidx.media3:media3-common:$media3Version")

    // Compose
    implementation(platform("androidx.compose:compose-bom:2026.06.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Material Components (Required for Theme.MaterialComponents in themes.xml)
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.14.0")

    // JNA for UniFFI bindings
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    // Testing
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation(platform("androidx.compose:compose-bom:2026.06.01"))
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

// The connected-ffi package keeps its debug symbols (see
// [profile.release.package.connected-ffi] in the workspace Cargo.toml) so
// that uniffi-bindgen can extract the FFI metadata at build time. Those
// symbols inflate the .so files in jniLibs by ~30-50% which is unnecessary
// at runtime. Once generateBindingsRelease has read the metadata, we strip
// the jniLibs copies so the APK ships the smallest possible binaries while
// the target/ copies (used only by the build) stay intact.
val hostTag = run {
    val os = System.getProperty("os.name").lowercase()
    when {
        os.contains("mac") -> "darwin-x86_64"
        os.contains("linux") -> "linux-x86_64"
        os.contains("win") -> "windows-x86_64"
        else -> "linux-x86_64"
    }
}
val llvmStrip = File(ndkDir, "toolchains/llvm/prebuilt/$hostTag/bin/llvm-strip")
val jniLibsDir = file("${project.projectDir}/src/main/jniLibs")
val cargoTargetDir = file("${project.rootDir}/../target")
// Mapping from Android ABI name to cargo target triple (used in target/).
val abiToTargetTriple = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7-linux-androideabi",
    "x86_64" to "x86_64-linux-android",
    "x86" to "i686-linux-android",
)
val abiList = abiToTargetTriple.keys.toList()

tasks.register<Exec>("stripRustJniLibs") {
    description = "Strip debug symbols from the jniLibs copies of libconnected_ffi.so."
    group = "build-setup"
    val soFiles = abiList.map { File(jniLibsDir, "$it/libconnected_ffi.so") }
    val llvmStripPath = llvmStrip.absolutePath
    inputs.files(soFiles)
    outputs.files(soFiles)
    // The .so files are produced by buildRustRelease, which runs before
    // this finalizer but after configuration. Resolve the file list at
    // execution time so a clean build (where nothing exists at config
    // time) still strips the freshly-built artifacts.
    executable = llvmStripPath
    onlyIf { soFiles.any { it.exists() } }
    doFirst {
        args = listOf("--strip-unneeded") + soFiles.filter { it.exists() }.map { it.absolutePath }
    }
}

// Once uniffi-bindgen has extracted the FFI metadata from the target/
// copies of libconnected_ffi.so, those copies are no longer needed for the
// build and can be stripped to reclaim ~24 MB of disk on the build machine.
// This has no effect on the APK (which uses the jniLibs copies).
tasks.register<Exec>("stripRustTarget") {
    description = "Strip debug symbols from the target/ copies of libconnected_ffi.so after uniffi-bindgen has run."
    group = "build-setup"
    val soFiles = abiToTargetTriple.values.flatMap { triple ->
        listOf("release", "debug").map { profile ->
            File(cargoTargetDir, "$triple/$profile/libconnected_ffi.so")
        }
    }
    val llvmStripPath = llvmStrip.absolutePath
    inputs.files(soFiles)
    outputs.files(soFiles)
    // Resolve the file list at execution time: on a clean build the .so
    // files are created by buildRust{Debug,Release} -> generateBindings*
    // and don't exist at configuration time, so a config-time filter is
    // always empty and llvm-strip errors with "no input file specified".
    executable = llvmStripPath
    onlyIf { soFiles.any { it.exists() } }
    doFirst {
        args = listOf("--strip-unneeded") + soFiles.filter { it.exists() }.map { it.absolutePath }
    }
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

// UniFFI-generated bindings reference the integrity/lib objects purely for
// class-loader side effects, which the Kotlin compiler flags as
// "Expression is unused". A wide variety of other IDE/compiler warnings
// (redundant public modifier, redundant Unit return, snake_case FFI symbol
// names, etc.) are also emitted by the generator. The :app:cleanUniffiBindings
// task runs scripts/clean_uniffi_bindings.py after every regen, applying
// the safe in-place transforms that don't touch the FFI ABI (drops `public`,
// drops `: Unit`, drops `kotlin.` qualifiers, strips redundant backticks,
// simplifies `{ -> }` zero-arg lambdas, removes redundant `Unit` statements,
// etc.) and prepending a comprehensive @file:Suppress(...) header that
// covers everything that can't be fixed in-place (FFI symbol names, unused
// FFI stubs, false-positive spell-checker hits on "uniffi", class-naming
// conventions, etc.).
//
// It is registered as a finalizer of the generateBindings tasks below so
// the cleanup runs automatically after every regen. The task is an Exec
// task (no inline doLast closures) so the build stays compatible with
// Gradle's configuration cache.
val generatedBindingsFile =
    file("${project.projectDir}/src/main/kotlin/uniffi/connected_ffi/connected_ffi.kt")
val uniffiCleanerScript = file("${project.rootDir}/../scripts/clean_uniffi_bindings.py")

tasks.register<Exec>("cleanUniffiBindings") {
    description = "Clean the UniFFI-generated Kotlin bindings and prepend a comprehensive @file:Suppress header."
    group = "build-setup"
    // The task only ever runs as a finalizer of generateBindings{,Release},
    // so the bindings file is guaranteed to exist by the time we get here.
    // No onlyIf guard is used because File-derived predicates are not
    // serializable into the configuration cache.
    commandLine(
        "python3",
        uniffiCleanerScript.absolutePath,
        generatedBindingsFile.absolutePath,
    )
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
    tasks.matching { it.name == "generateBindings" || it.name == "generateBindingsRelease" }
        .configureEach {
            finalizedBy("cleanUniffiBindings", "stripRustTarget")
        }
    tasks.matching { it.name.startsWith("pre") && it.name.endsWith("DebugBuild") }.configureEach {
        dependsOn("generateBindings")
    }
    tasks.matching { it.name.startsWith("pre") && it.name.endsWith("ReleaseBuild") }.configureEach {
        dependsOn("generateBindingsRelease")
    }
    // The release Rust build copies unstripped .so files into jniLibs (the
    // symbols are needed by uniffi-bindgen via the target/ copy, not jniLibs).
    // After the build, strip the jniLibs copies so the APK ships the smallest
    // possible binaries. Done as a finalizer so it runs after cargo ndk
    // copies the artifacts, and is skipped on incremental builds where the
    // .so files haven't changed.
    tasks.matching { it.name == "buildRustRelease" }.configureEach {
        finalizedBy("stripRustJniLibs")
    }
    tasks.matching { it.name.startsWith("merge") && it.name.endsWith("JniLibFolders") }
        .configureEach {
            dependsOn("stripRustJniLibs")
        }
}
