// SPDX-License-Identifier: GPL-3.0-or-later
// A tiny observable store (no dependency): one immutable state object, `set` to
// change it, `useStore(selector)` to read a slice in a component. Components re-render
// only when their selected slice changes (shallow comparison for objects/arrays).
import { useEffect, useRef, useState } from "preact/hooks";

export type Listener<S> = (state: S, prev: S) => void;

export class Store<S extends object> {
  private state: S;
  private listeners = new Set<Listener<S>>();

  constructor(initial: S) {
    this.state = initial;
  }

  get(): S {
    return this.state;
  }

  /** Shallow-merges a partial state (or the result of a function of the current state). */
  set(change: Partial<S> | ((s: S) => Partial<S>)): void {
    const prev = this.state;
    const patch = typeof change === "function" ? change(prev) : change;
    let changed = false;
    for (const k of Object.keys(patch) as (keyof S)[]) {
      if (prev[k] !== patch[k]) {
        changed = true;
        break;
      }
    }
    if (!changed) return;
    this.state = { ...prev, ...patch };
    for (const l of [...this.listeners]) l(this.state, prev);
  }

  subscribe(l: Listener<S>): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }
}

function shallowEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.is((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k])) return false;
  }
  return true;
}

/** Reads `selector(state)` and re-renders when it changes (shallowly). */
export function useSelector<S extends object, T>(store: Store<S>, selector: (s: S) => T): T {
  const [, force] = useState(0);
  const sel = useRef(selector);
  sel.current = selector;
  const value = selector(store.get());
  const last = useRef(value);
  last.current = value;
  useEffect(() => {
    const check = () => {
      const next = sel.current(store.get());
      if (!shallowEqual(next, last.current)) {
        last.current = next;
        force((n) => n + 1);
      }
    };
    const unsub = store.subscribe(check);
    check(); // state may have changed between render and subscription
    return unsub;
  }, [store]);
  return value;
}
