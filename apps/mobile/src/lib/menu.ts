import { ActionSheetIOS, Alert, Platform } from 'react-native';

export interface MenuItem {
  label: string;
  onPress: () => void;
  destructive?: boolean;
  /** Shows a checkmark (current choice). */
  checked?: boolean;
}

/** Native action sheet on iOS, an alert list on Android. */
export function showMenu(title: string | undefined, items: MenuItem[], message?: string) {
  const labels = items.map((i) => (i.checked ? `✓ ${i.label}` : i.label));
  if (Platform.OS === 'ios') {
    const destructiveButtonIndex = items.map((i, n) => (i.destructive ? n : -1)).filter((n) => n >= 0);
    ActionSheetIOS.showActionSheetWithOptions(
      { title, message, options: [...labels, 'Cancel'], cancelButtonIndex: items.length, destructiveButtonIndex },
      (i) => { items[i]?.onPress(); },
    );
    return;
  }
  Alert.alert(title ?? '', message, [
    ...items.map((i, n) => ({ text: labels[n]!, onPress: i.onPress, style: i.destructive ? ('destructive' as const) : ('default' as const) })),
    { text: 'Cancel', style: 'cancel' as const },
  ]);
}
