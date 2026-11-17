// SPDX-License-Identifier: GPL-3.0-or-later
// Motion and visibility helpers for the skin.
//
// The app setting `ui.motion` is "system" | "reduce" | "full" (docs/APP_API.md
// section 5). The skin components take the resolved value ("full" | "reduce");
// `useResolvedMotion` does the resolution, following the operating system's
// reduce-motion preference live when the setting is "system".

import { useEffect, useState } from "preact/hooks";

export type MotionSetting = "system" | "reduce" | "full";
export type Motion = "full" | "reduce";

const REDUCE_QUERY = "(prefers-reduced-motion: reduce)";

/** Pure resolution rule (unit-tested). */
export function resolveMotion(setting: MotionSetting, systemPrefersReduce: boolean): Motion {
  if (setting === "system") return systemPrefersReduce ? "reduce" : "full";
  return setting;
}

function systemPrefersReduce(): boolean {
  return typeof matchMedia === "function" && matchMedia(REDUCE_QUERY).matches;
}

/** True while the OS asks for reduced motion; updates live. */
export function useSystemReducedMotion(): boolean {
  const [reduce, setReduce] = useState(systemPrefersReduce);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia(REDUCE_QUERY);
    const onChange = () => setReduce(mq.matches);
    // addEventListener on MediaQueryList: Safari 14+, Chrome 45+ (baseline Safari 16.4).
    mq.addEventListener("change", onChange);
    onChange();
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return reduce;
}

/** The app setting resolved to what the skin should do right now. */
export function useResolvedMotion(setting: MotionSetting): Motion {
  const reduce = useSystemReducedMotion();
  return resolveMotion(setting, reduce);
}

function pageVisible(): boolean {
  return typeof document === "undefined" || document.visibilityState !== "hidden";
}

/**
 * False while the page is hidden: a background tab, or a minimised / fully covered
 * window when the web view reports it as hidden. Not every host does (WebView2 only
 * when its controller is hidden), so the frame also takes an explicit `paused` prop.
 */
export function usePageVisible(): boolean {
  const [visible, setVisible] = useState(pageVisible);
  useEffect(() => {
    const onChange = () => setVisible(pageVisible());
    document.addEventListener("visibilitychange", onChange);
    onChange();
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  return visible;
}
