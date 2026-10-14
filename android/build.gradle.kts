plugins {
    id("com.android.application") version "9.4.1" apply false
    id("com.android.library") version "9.4.1" apply false
<<<<<<< HEAD
    id("org.jetbrains.kotlin.android") version "2.4.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.20" apply false
=======
    id("org.jetbrains.kotlin.android") version "2.4.20" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
>>>>>>> origin/main
}

tasks.register("clean", Delete::class) {
    delete(rootProject.layout.buildDirectory)
}
