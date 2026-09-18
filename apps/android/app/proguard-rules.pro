# Keep kotlinx.serialization generated serializers.
-keepclassmembers class ** {
    *** Companion;
}
-keepclasseswithmembers class ** {
    kotlinx.serialization.KSerializer serializer(...);
}
-keep,includedescriptorclasses class sh.cosmos.companion.**$$serializer { *; }
-keepclassmembers class sh.cosmos.companion.** {
    *** Companion;
}
