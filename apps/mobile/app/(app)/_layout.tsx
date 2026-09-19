import { Tabs } from "expo-router";
import { useColorScheme } from "react-native";
import { palette } from "../../src/ui/theme";

export default function AppTabs() {
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  return (
    <Tabs
      screenOptions={{
        headerShown: false,
        tabBarStyle: {
          backgroundColor: c.surface,
          borderTopColor: c.border,
        },
        tabBarActiveTintColor: c.accent,
        tabBarInactiveTintColor: c.faint,
      }}
    >
      <Tabs.Screen name="index" options={{ title: "Sessions" }} />
      <Tabs.Screen name="devices" options={{ title: "Desktops" }} />
      <Tabs.Screen name="computer" options={{ title: "Computer" }} />
      <Tabs.Screen name="settings" options={{ title: "Settings" }} />
      <Tabs.Screen name="new" options={{ href: null, title: "New" }} />
    </Tabs>
  );
}
