# ProGuard rules for Connected app

# Keep all Android permission-related classes
-keep class android.Manifest$permission { *; }

# Keep NotificationListenerService and related classes
-keep class * extends android.service.notification.NotificationListenerService {
    *;
}
-keep class com.connected.app.MediaObserverService { *; }

# Keep BroadcastReceivers
-keep class * extends android.content.BroadcastReceiver {
    *;
}
-keep class com.connected.app.TransferActionReceiver { *; }
-keep class com.connected.app.MediaControlReceiver { *; }

# Keep TelephonyProvider for permission handling
-keep class com.connected.app.TelephonyProvider { *; }

# Keep Services
-keep class com.connected.app.ConnectedService { *; }

# Keep Activities
-keep class com.connected.app.MainActivity { *; }
-keep class com.connected.app.ClipboardHelperActivity { *; }

# JNA for UniFFI bindings
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }

# UniFFI generated code
-keep class uniffi.** { *; }
-keepclassmembers class uniffi.** { *; }

# Keep native method names
-keepclasseswithmembernames class * {
    native <methods>;
}

# Keep Parcelables
-keepclassmembers class * implements android.os.Parcelable {
    public static final android.os.Parcelable$Creator CREATOR;
}

# Keep classes that are accessed via reflection for permissions
-keepclassmembers class * {
    @android.annotation.RequiresPermission *;
}

# Keep ComponentName for NotificationListenerService registration check
-keepnames class * extends android.service.notification.NotificationListenerService

# Preserve line numbers for debugging crashes
-keepattributes SourceFile,LineNumberTable

# Keep annotations
-keepattributes *Annotation*

# Keep Serializable classes
-keepclassmembers class * implements java.io.Serializable {
    static final long serialVersionUID;
    private static final java.io.ObjectStreamField[] serialPersistentFields;
    private void writeObject(java.io.ObjectOutputStream);
    private void readObject(java.io.ObjectInputStream);
    java.lang.Object writeReplace();
    java.lang.Object readResolve();
}

# Kotlin specific
-keep class kotlin.Metadata { *; }
-keepclassmembers class kotlin.Metadata {
    public <methods>;
}

# Compose
-keep class androidx.compose.** { *; }
-keepclassmembers class androidx.compose.** { *; }

# Keep MediaSession classes for media controls
-keep class android.media.session.** { *; }
-keep class android.support.v4.media.** { *; }
-keep class androidx.media.** { *; }
