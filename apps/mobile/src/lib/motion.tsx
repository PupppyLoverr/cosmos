import React, { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import { enterBezier, motion } from './theme';

/** Desktop `Motion::flavourWords` — rotated every 7s, seeded per chat. */
const FLAVOUR_WORDS: string[] = [
  'Cosmosing', 'Thinking', 'Pondering', 'Scheming', 'Brewing', 'Weaving',
  'Tinkering', 'Musing', 'Composing', 'Sifting', 'Untangling', 'Distilling',
  'Sketching', 'Plotting', 'Riffing', 'Percolating', 'Noodling', 'Conjuring',
];
const FLAVOUR_ROTATE_SECS = 7;

function flavourSeed(chatId: string): number {
  let h = 0xcbf29ce484222325n;
  for (const b of chatId) { h = (h ^ BigInt(b.charCodeAt(0))) & 0xffffffffffffffffn; h = (h * 0x100000001b3n) & 0xffffffffffffffffn; }
  return Number(h);
}

/** Rotating flavour word for a live session, matching the desktop's WorkingIndicator. */
export function useFlavourWord(chatId: string, active: boolean): string {
  const seed = useMemo(() => flavourSeed(chatId), [chatId]);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, [active]);
  if (!active) return '';
  return FLAVOUR_WORDS[(seed + Math.floor(tick / FLAVOUR_ROTATE_SECS)) % FLAVOUR_WORDS.length]!;
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

export const enterEasing = Easing.bezier(enterBezier[0], enterBezier[1], enterBezier[2], enterBezier[3]);

/** One-shot message entrance: desktop fade-in (translateY 4→0), shortened for the phone. */
export function Arrive({ active, children }: { active: boolean; children: React.ReactNode }) {
  const reduce = useReduceMotion();
  const opacity = useSharedValue(active && !reduce ? 0 : 1);
  const y = useSharedValue(active && !reduce ? 4 : 0);
  useEffect(() => {
    if (!active || reduce) {
      opacity.set(1);
      y.set(0);
      return;
    }
    const timing = { duration: motion.fadeIn, easing: enterEasing };
    opacity.set(withTiming(1, timing));
    y.set(withTiming(0, timing));
  }, [active, reduce, opacity, y]);
  const style = useAnimatedStyle(() => ({
    opacity: opacity.get(),
    transform: [{ translateY: y.get() }],
  }));
  return <Animated.View style={style}>{children}</Animated.View>;
}
