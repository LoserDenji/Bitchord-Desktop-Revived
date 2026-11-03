import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("org.jetbrains.kotlin.multiplatform")
    id("org.jetbrains.kotlin.plugin.serialization")
}

kotlin {
    jvm {
        compilerOptions {
            jvmTarget.set(JvmTarget.JVM_17)
        }
    }

    sourceSets {
        // The desktop application's data layer. Kept as its own source set so the
        // shared/common models stay free of JVM-only APIs.
        val jvmSharedMain by creating {
            dependsOn(commonMain.get())
            dependencies {
                // Compile-only: the application supplies the runtime artifact itself.
                compileOnly("com.microsoft.onnxruntime:onnxruntime:1.20.0")
                // The network stack the lyrics providers and the YouTube Music
                // client run on.
                api("io.ktor:ktor-client-core:3.5.2")
                api("io.ktor:ktor-client-okhttp:3.5.2")
                api("io.ktor:ktor-client-websockets:3.5.2")
                api("io.ktor:ktor-client-content-negotiation:3.5.2")
                api("io.ktor:ktor-serialization-kotlinx-json:3.5.2")
                api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.11.0")
                api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
                api("org.jsoup:jsoup:1.22.2")
                // YouTube stream resolution: InnerTubeX's client catalog and cipher
                // tiers, NewPipe as the fallback. NewPipe is compile-only because the
                // application ships a trimmed jar of it.
                api("com.github.MetrolistGroup.innertubex:innertubex:v0.7.0")
                compileOnly("com.github.TeamNewPipe:NewPipeExtractor:v0.26.3")
            }
        }
        jvmMain.get().dependsOn(jvmSharedMain)

        commonMain.dependencies {
            // Models in this module intentionally have no Compose dependency.
            implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
        jvmTest.dependencies {
            implementation("com.squareup.okhttp3:mockwebserver:5.3.2")
        }
    }
}
