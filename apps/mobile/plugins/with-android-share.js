const { withAndroidManifest, withMainActivity } = require("@expo/config-plugins");

/** Keep the Android SHARE / SEND intent filter so gallery Share → Cosmos works. */
function withAndroidShareFilter(config) {
  return withAndroidManifest(config, (config) => {
    const app = config.modResults.manifest.application?.[0];
    const activity = app?.activity?.find(
      (item) => item.$?.["android:name"] === ".MainActivity",
    );
    if (!activity) return config;
    activity["intent-filter"] = activity["intent-filter"] || [];
    const hasSend = activity["intent-filter"].some((filter) =>
      (filter.action || []).some((a) => a.$?.["android:name"] === "android.intent.action.SEND"),
    );
    if (!hasSend) {
      activity["intent-filter"].push({
        action: [{ $: { "android:name": "android.intent.action.SEND" } }],
        category: [{ $: { "android:name": "android.intent.category.DEFAULT" } }],
        data: [
          { $: { "android:mimeType": "image/*" } },
          { $: { "android:mimeType": "*/*" } },
        ],
      });
    }
    return config;
  });
}

/** Route ACTION_SEND extras onto cosmos://share so JS Linking can stage bytes. */
function withShareMainActivity(config) {
  return withMainActivity(config, (config) => {
    if (config.modResults.language !== "kt") return config;
    let src = config.modResults.contents;
    if (src.includes("consumeCosmosShare")) return config;
    if (!src.includes("import android.content.Intent")) {
      src = src.replace(
        "import android.os.Bundle",
        "import android.content.Intent\nimport android.net.Uri\nimport android.os.Bundle",
      );
    }
    if (!src.includes("override fun onNewIntent")) {
      src = src.replace(
        "override fun onCreate(savedInstanceState: Bundle?) {",
        `override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    consumeCosmosShare(intent)
  }

  override fun onCreate(savedInstanceState: Bundle?) {`,
      );
    }
    if (!src.includes("consumeCosmosShare(intent)")) {
      src = src.replace(
        "super.onCreate(null)",
        "super.onCreate(null)\n    consumeCosmosShare(intent)",
      );
    }
    if (!src.includes("private fun consumeCosmosShare")) {
      const method = `
  private fun consumeCosmosShare(intent: Intent?) {
    if (intent == null) return
    if (intent.action != Intent.ACTION_SEND) return
    val stream: Uri? = if (Build.VERSION.SDK_INT >= 33) {
      intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
    } else {
      @Suppress("DEPRECATION")
      intent.getParcelableExtra(Intent.EXTRA_STREAM)
    }
    val text = intent.getStringExtra(Intent.EXTRA_TEXT)
    val target = when {
      stream != null -> Uri.parse("cosmos://share").buildUpon()
        .appendQueryParameter("uri", stream.toString()).build()
      !text.isNullOrEmpty() -> Uri.parse("cosmos://share").buildUpon()
        .appendQueryParameter("text", text).build()
      else -> null
    }
    if (target != null) {
      intent.data = target
    }
  }
`;
      const idx = src.lastIndexOf("}");
      src = src.slice(0, idx) + method + "\n" + src.slice(idx);
    }
    config.modResults.contents = src;
    return config;
  });
}

function withAndroidShare(config) {
  return withShareMainActivity(withAndroidShareFilter(config));
}

module.exports = withAndroidShare;
