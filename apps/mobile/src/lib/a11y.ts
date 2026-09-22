import { useWindowDimensions, AccessibilityInfo } from 'react-native';
import { useEffect, useState } from 'react';

export const HIT_SLOP_44 = (visual: number) => Math.max(0, Math.ceil((44 - visual) / 2));

export function useFontScale(): number {
  return useWindowDimensions().fontScale;
}

export function useLargeText(): boolean {
  return useFontScale() >= 1.3;
}

export function useReduceMotion(): boolean {
  const [reduce, setReduce] = useState(false);
  useEffect(() => {
    let live = true;
    void AccessibilityInfo.isReduceMotionEnabled().then((value) => {
      if (live) setReduce(value);
    });
    const sub = AccessibilityInfo.addEventListener('reduceMotionChanged', setReduce);
    return () => {
      live = false;
      sub.remove();
    };
  }, []);
  return reduce;
}
