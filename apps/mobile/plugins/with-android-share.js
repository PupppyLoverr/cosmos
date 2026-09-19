const { withAndroidManifest } = require("@expo/config-plugins");

/** Keep the Android SHARE / SEND intent filter so gallery Share → Cosmos works. */
function withAndroidShare(config) {
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

module.exports = withAndroidShare;
