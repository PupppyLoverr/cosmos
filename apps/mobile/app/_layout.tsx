import { Stack } from "expo-router";
import { StatusBar } from "expo-status-bar";
import { useColorScheme } from "react-native";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { AppProvider } from "../src/state/store";
import { palette } from "../src/ui/theme";

export default function RootLayout() {
  const scheme = useColorScheme() === "light" ? "light" : "dark";
  const c = palette(scheme);
  return (
    <GestureHandlerRootView style={{ flex: 1, backgroundColor: c.bg }}>
      <AppProvider>
        <StatusBar style={scheme === "light" ? "dark" : "light"} />
        <Stack
          screenOptions={{
            headerShown: false,
            contentStyle: { backgroundColor: c.bg },
            animation: "ios_from_right",
          }}
        />
      </AppProvider>
    </GestureHandlerRootView>
  );
}
