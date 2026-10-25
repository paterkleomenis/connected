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

val latestNdkVersion: String = ndkDir?.name ?: "26.3.11579223"

// Helper for NDK-dependent tasks
val effectiveNdkDir: File = ndkDir ?: File(sdkDir, "ndk/$latestNdkVersion")

// Native libraries are built into the Gradle build directory, never into the
// source tree. This lets Gradle track cargo-ndk's outputs and invalidate the
// merge/packaging tasks when a library changes.
val workspaceRoot = file("${project.rootDir}/..")
val rustJniLibsRoot = layout.buildDirectory.dir("rustJniLibs")
val rustJniLibsDebugDir = rustJniLibsRoot.map { it.dir("debug") }
val rustJniLibsReleaseDir = rustJniLibsRoot.map { it.dir("release") }

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
        versionCode = 30504
        versionName = "3.5.4"

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
        create("playStore") {
            dimension = "distribution"
        }
        create("sideload") {
            dimension = "distribution"
            isDefault = true
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
        buildConfig = true
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

    // F-Droid rejects APKs containing the Google-encrypted "Dependency metadata"
    // signing block that AGP adds by default for Play Console SDK insights.
    // Disable it for both APKs and Bundles. See:
    // https://developer.android.com/build/dependencies#dependency-information-play-console
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    // Native libraries come from the cargo-ndk build directory, not from the
    // source tree. main's default src/main/jniLibs is cleared so a stale copy
    // left by older builds can never be packaged.
    sourceSets {
        getByName("main") {
            jniLibs.directories.clear()
        }
        getByName("debug") {
            jniLibs.directories.add(rustJniLibsDebugDir.get().asFile.absolutePath)
        }
        getByName("release") {
            jniLibs.directories.add(rustJniLibsReleaseDir.get().asFile.absolutePath)
        }
    }
}

dependencies {
    implementation("androidx.exifinterface:exifinterface:1.4.2")
    // Core library desugaring for Java 21 APIs on older Android
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")

    // AndroidX Core
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.11.0")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.documentfile:documentfile:1.1.0")

    // Media3
    val media3Version = "1.11.1"
    implementation("androidx.media3:media3-session:$media3Version")
    implementation("androidx.media3:media3-common:$media3Version")

    // Compose
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    // Material Components (Required for Theme.MaterialComponents in themes.xml)
    implementation("androidx.appcompat:appcompat:1.8.0")
    implementation("com.google.android.material:material:1.14.0")

    // JNA for UniFFI bindings
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    // Testing
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    androidTestImplementation(platform("androidx.compose:compose-bom:2026.09.00"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}

// Everything that affects the connected-ffi cdylib. Declaring these as task
// inputs lets Gradle skip cargo when nothing changed and rerun it (and
// resnapshot the outputs) when a Rust source or manifest changes.
val rustInputs = files(
    workspaceRoot.resolve("Cargo.toml"),
    workspaceRoot.resolve("Cargo.lock"),
    workspaceRoot.resolve("rust-toolchain.toml"),
    fileTree(workspaceRoot) {
        include("core/**/*.rs", "core/**/Cargo.toml", "ffi/**/*.rs", "ffi/**/Cargo.toml", ".cargo/**")
        exclude("**/target/**")
    },
)

// Task to build Rust library for Android
tasks.register<Exec>("buildRustDebug") {
    workingDir = file("${project.rootDir}/../ffi")
    environment("ANDROID_NDK_HOME", effectiveNdkDir.absolutePath)
    val outputDir = rustJniLibsDebugDir.get().asFile
    commandLine(cargoExecutable, "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-t", "x86_64",
        "-t", "x86",
        "-o", outputDir.absolutePath,
        "build"
    )
    inputs.files(rustInputs)
    outputs.dir(rustJniLibsDebugDir)
    // cargo-ndk only copies when the cargo artifact is newer than the
    // destination. Clear the destination so a stale or tampered file in the
    // build directory can never survive a rebuild.
    doFirst { outputDir.deleteRecursively() }
}

tasks.register<Exec>("buildRustRelease") {
    workingDir = file("${project.rootDir}/../ffi")
    environment("ANDROID_NDK_HOME", effectiveNdkDir.absolutePath)
    val outputDir = rustJniLibsReleaseDir.get().asFile
    commandLine(cargoExecutable, "ndk",
        "-t", "arm64-v8a",
        "-t", "armeabi-v7a",
        "-t", "x86_64",
        "-t", "x86",
        "-o", outputDir.absolutePath,
        "build", "--release"
    )
    inputs.files(rustInputs)
    outputs.dir(rustJniLibsReleaseDir)
    doFirst { outputDir.deleteRecursively() }
}

val hostOsName = System.getProperty("os.name")
val isWindowsHost = hostOsName.startsWith("Windows", ignoreCase = true)
val isMacHost = hostOsName.startsWith("Mac", ignoreCase = true)
val cargoTargetDir = file("${project.rootDir}/../target")
val hostFfiLib = File(
    cargoTargetDir,
    when {
        isWindowsHost -> "debug/connected_ffi.dll"
        isMacHost -> "debug/libconnected_ffi.dylib"
        else -> "debug/libconnected_ffi.so"
    },
)
// Mapping from Android ABI name to cargo target triple (used in target/).
val abiToTargetTriple = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7-linux-androideabi",
    "x86_64" to "x86_64-linux-android",
    "x86" to "i686-linux-android",
)
// The target/ copies are only intermediates for the cargo ndk build that
// populates the jniLibs output directory. Delete them after the Rust build so
// they don't linger on disk; bindgen reads the host library instead.
tasks.register<Delete>("removeRustTargetLibs") {
    val soFiles = abiToTargetTriple.values.flatMap { triple ->
        listOf("release", "debug").map { profile ->
            File(cargoTargetDir, "$triple/$profile/libconnected_ffi.so")
        }
    }
    delete(soFiles)
}

// Build the host FFI library used as the UniFFI metadata source. The
// generated bindings are architecture-independent, so this is shared by the
// debug and release Android variants and does not depend on the Android
// cross-compilation.
tasks.register<Exec>("buildFfiHost") {
    workingDir = file("${project.rootDir}/..")
    commandLine(cargoExecutable, "build", "-p", "connected-ffi")
}

// Generate UniFFI Kotlin bindings (using bundled uniffi-bindgen)
tasks.register<Exec>("generateBindings") {
    workingDir = file("${project.rootDir}/..")
    commandLine(cargoExecutable, "run",
        "-p", "connected-ffi",
        "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", hostFfiLib.absolutePath,
        "--language", "kotlin",
        "--out-dir", "${project.projectDir}/src/main/kotlin",
        "--no-format"
    )
    dependsOn("buildFfiHost")
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
        if (isWindowsHost) "python" else "python3",
        uniffiCleanerScript.absolutePath,
        generatedBindingsFile.absolutePath,
    )
}

tasks.register<Exec>("generateBindingsRelease") {
    workingDir = file("${project.rootDir}/..")
    commandLine(cargoExecutable, "run",
        "-p", "connected-ffi",
        "--bin", "uniffi-bindgen",
        "--",
        "generate",
        "--library", hostFfiLib.absolutePath,
        "--language", "kotlin",
        "--out-dir", "${project.projectDir}/src/main/kotlin",
        "--no-format"
    )
    dependsOn("buildFfiHost")
}

afterEvaluate {
    tasks.matching { it.name == "generateBindings" || it.name == "generateBindingsRelease" }
        .configureEach {
            finalizedBy("cleanUniffiBindings")
        }
    tasks.matching { it.name == "buildRustDebug" || it.name == "buildRustRelease" }
        .configureEach {
            finalizedBy("removeRustTargetLibs")
        }
    tasks.matching { it.name.startsWith("pre") && it.name.endsWith("DebugBuild") }.configureEach {
        dependsOn("generateBindings", "buildRustDebug")
    }
    tasks.matching { it.name.startsWith("pre") && it.name.endsWith("ReleaseBuild") }.configureEach {
        dependsOn("generateBindingsRelease", "buildRustRelease")
    }
}
