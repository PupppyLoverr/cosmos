// Adopts the UIScene life cycle required by the iOS 27 SDK. The Expo 57 template still uses the
// app-delegate life cycle, so apps built with Xcode 27 abort at launch; Expo ships
// `ExpoAppSceneDelegate` for exactly this case.
const { withAppDelegate, withInfoPlist } = require('expo/config-plugins');

const SCENE_DELEGATE = `
class SceneDelegate: ExpoAppSceneDelegate {}
`;

const FACTORY_PROVIDER = 'class AppDelegate: ExpoAppDelegate, ExpoReactNativeFactoryProvider {';

module.exports = function withSceneLifecycle(config) {
  config = withInfoPlist(config, (c) => {
    c.modResults.UIApplicationSceneManifest = {
      UIApplicationSupportsMultipleScenes: false,
      UISceneConfigurations: {
        UIWindowSceneSessionRoleApplication: [
          {
            UISceneConfigurationName: 'Default Configuration',
            UISceneDelegateClassName: '$(PRODUCT_MODULE_NAME).SceneDelegate',
          },
        ],
      },
    };
    return c;
  });

  config = withAppDelegate(config, (c) => {
    let src = c.modResults.contents;
    if (src.includes('SceneDelegate')) return c;
    src = src.replace('class AppDelegate: ExpoAppDelegate {', FACTORY_PROVIDER);
    // The scene delegate owns the window; the app delegate only builds the factory.
    src = src.replace(
      /#if os\(iOS\) \|\| os\(tvOS\)\n\s*window = UIWindow\(frame: UIScreen\.main\.bounds\)\n\s*factory\.startReactNative\([\s\S]*?\)\n#endif\n/,
      '',
    );
    src += SCENE_DELEGATE;
    c.modResults.contents = src;
    return c;
  });

  return config;
};
