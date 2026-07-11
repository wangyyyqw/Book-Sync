# Rust JNI locates these entry points and callback methods by their exact JVM names.
-keep class com.kmosync.KmoSyncJni { *; }
-keep interface com.kmosync.NativeEventCallback {
    void onEvent(int, java.lang.String);
}
-keepclasseswithmembers,allowoptimization class * implements com.kmosync.NativeEventCallback {
    void onEvent(int, java.lang.String);
}
