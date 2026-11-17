// SPDX-License-Identifier: GPL-3.0-or-later
// Small shared controls with consistent markup and accessible labels. Plain styling
// (style.css); the skin restyles them through the class names.
import type { ComponentChildren } from "preact";
import { useRef } from "preact/hooks";

let uid = 0;
/** A stable id for label/for pairs. */
export function useId(prefix: string): string {
  const r = useRef<string>("");
  if (!r.current) r.current = `${prefix}-${++uid}`;
  return r.current;
}

export function Section(props: { title: string; class?: string; children: ComponentChildren; actions?: ComponentChildren }) {
  return (
    <section class={`panel ${props.class ?? ""}`} aria-label={props.title}>
      <header class="panel__header">
        <h2 class="panel__title">{props.title}</h2>
        {props.actions && <div class="panel__actions">{props.actions}</div>}
      </header>
      {props.children}
    </section>
  );
}

/** A labelled range input. `format` turns the value into the text shown next to it. */
export function Slider(props: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onInput: (v: number) => void;
  format?: (v: number) => string;
  disabled?: boolean;
  class?: string;
}) {
  const id = useId("slider");
  const shown = props.format ? props.format(props.value) : String(props.value);
  return (
    <div class={`field field--slider ${props.class ?? ""}`}>
      <label for={id} class="field__label">
        {props.label}
      </label>
      <input
        id={id}
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value}
        disabled={props.disabled}
        aria-valuetext={shown}
        onInput={(e) => props.onInput(Number((e.currentTarget as HTMLInputElement).value))}
      />
      <output for={id} class="field__value">
        {shown}
      </output>
    </div>
  );
}

export function Toggle(props: { label: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; hint?: string }) {
  const id = useId("toggle");
  return (
    <div class="field field--toggle">
      <input id={id} type="checkbox" role="switch" checked={props.checked} disabled={props.disabled} onChange={(e) => props.onChange((e.currentTarget as HTMLInputElement).checked)} />
      <label for={id} class="field__label">
        {props.label}
      </label>
      {props.hint && <span class="field__hint">{props.hint}</span>}
    </div>
  );
}

export function Select<T extends string>(props: { label: string; value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; disabled?: boolean }) {
  const id = useId("select");
  return (
    <div class="field field--select">
      <label for={id} class="field__label">
        {props.label}
      </label>
      <select id={id} value={props.value} disabled={props.disabled} onChange={(e) => props.onChange((e.currentTarget as HTMLSelectElement).value as T)}>
        {props.options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

/** A button that only shows a symbol: always carries an accessible name. */
export function IconButton(props: { label: string; symbol: string; onClick: () => void; class?: string; disabled?: boolean; pressed?: boolean; title?: string }) {
  return (
    <button
      type="button"
      class={`icon-button ${props.class ?? ""}`}
      aria-label={props.label}
      title={props.title ?? props.label}
      aria-pressed={props.pressed}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      <span aria-hidden="true">{props.symbol}</span>
    </button>
  );
}

/** A labelled progress bar (0..1). */
export function Progress(props: { value: number; label: string; class?: string }) {
  const v = Math.max(0, Math.min(1, props.value));
  return (
    <div class={`progress ${props.class ?? ""}`} role="progressbar" aria-label={props.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(v * 100)}>
      <div class="progress__bar" style={{ width: `${v * 100}%` }} />
    </div>
  );
}

/** A short explanation shown under a control. */
export function Note(props: { children: ComponentChildren; kind?: "info" | "warn" | "error" }) {
  return <p class={`note note--${props.kind ?? "info"}`}>{props.children}</p>;
}
