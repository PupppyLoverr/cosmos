import { Platform, type PressableAndroidRippleConfig } from "react-native";

/** Android Material ripple; iOS keeps opacity. */
export function androidRipple(color: string, borderless = false): PressableAndroidRippleConfig | undefined {
  if (Platform.OS !== "android") return undefined;
  return { color, borderless, foreground: true };
}

export const androidElevation = Platform.OS === "android" ? 4 : 0;
export const androidFabElevation = Platform.OS === "android" ? 6 : 0;
