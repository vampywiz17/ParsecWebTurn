plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.evoparseq.tv"
    compileSdk = 36
    defaultConfig {
        applicationId = "app.evoparseq.tv"
        minSdk = 29
        targetSdk = 36
        versionCode = 17
        versionName = "0.2.0-stream-preview.14"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a") }
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    packaging { jniLibs.useLegacyPackaging = false }
    lint { abortOnError = true }
}

dependencies {
    // Stable Compose 1.10.5 matches TV Material 1.1's 1.10.x API baseline.
    // Newer Compose 1.12 requires the separate API 37 / AGP 9 toolchain.
    implementation(platform("androidx.compose:compose-bom:2026.03.00"))
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.tv:tv-material:1.1.0")
}

// Do not produce an apparently installable APK with a missing Rust core.
tasks.register("verifyNativeLibraries") {
    doLast {
        listOf("arm64-v8a", "armeabi-v7a").forEach { abi ->
            check(file("src/main/jniLibs/$abi/libevoparseq_tv.so").isFile) {
                "Missing Rust library for $abi; run the cargo-ndk build in README.md first."
            }
        }
    }
}
tasks.named("preBuild") { dependsOn("verifyNativeLibraries") }
