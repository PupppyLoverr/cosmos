import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { Platform, useColorScheme } from "react-native";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { NavigationBar } from "expo-navigation-bar";
import { AppProvider } from "../src/state/store";
import { palette } from "../src/ui/theme";

export default function RootLayout() {
  const scheme = useColorScheme() === "light" ? "light" : "dark";
  const c = palette(scheme);
  return (
    <GestureHandlerRootView style={{ flex: 1, backgroundColor: c.bg }}>
      <AppProvider>
        <StatusBar style={scheme === "light" ? "dark" : "light"} />
        {Platform.OS === "android" ? <NavigationBar style={scheme === "light" ? "light" : "dark"} /> : null}
        <Stack
          screenOptions={{
            headerShown: false,
            contentStyle: { backgroundColor: c.bg },
            animation: Platform.OS === "android" ? "fade_from_bottom" : "ios_from_right",
          }}
        />
      </AppProvider>
    </GestureHandlerRootView>
  );
}
