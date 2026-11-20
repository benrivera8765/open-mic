// SPDX-License-Identifier: GPL-3.0-or-later
// The input picker (WO2 4.4, APP_API 12.4): every input device with its kind, the
// Bluetooth warning (never chosen automatically, WO2 trap 2), a level meter for each of
// its channels so the singer can pick the one that carries the voice (channels are never
// added together), plain advice when the voice is too quiet or clipping (the app never
// changes the input gain: that is the mixer's job), the permission state with the exact
// steps, and "watch only". Class hooks: .practice-picker, .practice-picker__device,
// .practice-picker__channel, .practice-meter.
import { useEffect, useState } from "preact/hooks";
import type { ChannelMeter, MicInput } from "../api/types";
import { formatRate } from "../lib/format";
import { chooseInput, closePicker, meter, openMicSettings, usePractice, watchOnly } from "../state/practice";

const KIND: Record<MicInput["kind"], string> = {
  usb: "USB",
  built_in: "Built-in",
  wired_external: "Wired",
  bluetooth: "Bluetooth",
  display: "Display",
  virtual: "Virtual",
  unknown: "Other",
};

/** A horizontal level meter: RMS bar, peak tick, -60..0 dBFS. */
function Meter({ m }: { m: ChannelMeter | null }) {
  const pos = (db: number) => `${Math.max(0, Math.min(100, ((db + 60) / 60) * 100)).toFixed(1)}%`;
  return (
    <span class={`practice-meter${m?.clipping ? " practice-meter--clip" : ""}`} role="meter" aria-valuemin={-60} aria-valuemax={0} aria-valuenow={m ? Math.round(m.rms_db) : -60} aria-label="Level">
      <span class="practice-meter__fill" style={{ width: m ? pos(m.rms_db) : "0%" }} />
      {m && <span class="practice-meter__peak" style={{ left: pos(m.peak_db) }} />}
      <span class="practice-meter__good" aria-hidden="true" />
    </span>
  );
}

function adviceText(m: ChannelMeter | null): string | null {
  if (!m) return null;
  if (m.message) return m.message;
  switch (m.advice) {
    case "too_quiet":
      return "Too quiet: turn up the microphone's gain on the mixer.";
    case "clipping":
      return "Clipping: turn the gain down on the mixer until the bar stays out of the red.";
    case "silent":
      return "Nothing on this channel.";
    case "waiting":
      return "Say something into the microphone…";
    default:
      return null;
  }
}

export function InputPicker() {
  const mics = usePractice((s) => s.mics);
  const levels = usePractice((s) => s.levels);
  const meterDevice = usePractice((s) => s.meterDevice);
  const perm = usePractice((s) => s.permission);
  const st = usePractice((s) => s.state);
  const [device, setDevice] = useState<string | null>(null);
  const [channel, setChannel] = useState<number | null>(null);

  // Start from the saved choice, else the automatic one.
  useEffect(() => {
    if (!mics || device !== null) return;
    setDevice(mics.chosen.device_id ?? mics.automatic ?? mics.inputs[0]?.id ?? null);
    setChannel(mics.chosen.channel);
  }, [mics]);

  const dev = mics?.inputs.find((i) => i.id === device) ?? null;
  const chans: ChannelMeter[] = levels && levels.device_id === device ? levels.channels : [];
  const suggested = levels && levels.device_id === device ? levels.suggested_channel : null;
  const voice = channel ?? suggested ?? 0;
  const denied = perm?.state === "denied" || perm?.state === "restricted";
  const windowsOff = perm?.windows && (perm.windows.microphone_access === "deny" || perm.windows.desktop_apps === "deny" || perm.windows.user_access === "deny");
  const voiceMeter = chans.find((c) => c.index === voice) ?? null;

  return (
    <div class="practice-panel" role="dialog" aria-modal="false" aria-label="Choose the input">
      <div class="practice-panel__box practice-panel__box--wide practice-picker">
        <h3 class="practice-panel__title">Microphone and voice channel</h3>
        {denied && (
          <div class="practice-note practice-note--inline" role="alert">
            <p>
              <strong>{windowsOff ? "Windows' microphone privacy switch is off." : "The microphone is not allowed."}</strong> {perm?.steps}
            </p>
            <button type="button" class="button button--small" onClick={() => void openMicSettings()}>
              Open the privacy settings
            </button>
          </div>
        )}
        {!mics ? (
          <p class="practice-panel__wait">Looking for inputs…</p>
        ) : mics.inputs.length === 0 ? (
          <p class="practice-panel__text">No microphone or audio input was found. Plug in your mixer (its USB cable) or a microphone; it appears here by itself.</p>
        ) : (
          <div class="practice-picker__cols">
            <fieldset class="practice-picker__devices">
              <legend>Input</legend>
              {mics.inputs.map((i) => (
                <label key={i.id} class={`practice-picker__device${i.id === device ? " practice-picker__device--on" : ""}${i.bluetooth ? " practice-picker__device--bt" : ""}`}>
                  <input
                    type="radio"
                    name="practice-input"
                    checked={i.id === device}
                    onChange={() => {
                      setDevice(i.id);
                      setChannel(null);
                      if (!denied) void meter(i.id);
                    }}
                  />
                  <span class="practice-picker__name">{i.name}</span>
                  <span class="practice-picker__tags">
                    <span class={`practice-tag practice-tag--${i.kind}`}>{KIND[i.kind]}</span>
                    <span class="practice-picker__fmt">
                      {i.channels} channel{i.channels === 1 ? "" : "s"} · {formatRate(i.sample_rate)}
                    </span>
                    {i.preferred && <span class="practice-tag practice-tag--ok">Recommended</span>}
                    {i.is_default && <span class="practice-tag">System default</span>}
                    {i.same_device_as_output && <span class="practice-tag">Also the output</span>}
                  </span>
                  {i.warning && <span class="practice-picker__warning">{i.warning}</span>}
                </label>
              ))}
            </fieldset>
            <fieldset class="practice-picker__channels" disabled={!dev || denied}>
              <legend>Voice channel</legend>
              {dev && chans.length === 0 && <p class="practice-panel__hint">{denied ? "Allow the microphone to see the levels." : meterDevice ? "Listening…" : "Choose an input to see its levels."}</p>}
              {chans.map((c) => (
                <label key={c.index} class={`practice-picker__channel${c.index === voice ? " practice-picker__channel--on" : ""}`}>
                  <input type="radio" name="practice-channel" checked={c.index === voice} onChange={() => setChannel(c.index)} onClick={() => setChannel(c.index)} />
                  <span class="practice-picker__chname">
                    Channel {c.index + 1}
                    {c.index === suggested && <span class="practice-tag practice-tag--ok">Voice?</span>}
                  </span>
                  <Meter m={c} />
                  <span class="practice-picker__db">{c.rms_db <= -119 ? "silent" : `${Math.round(c.rms_db)} dB`}</span>
                </label>
              ))}
              {voiceMeter && adviceText(voiceMeter) && <p class={`practice-picker__advice practice-picker__advice--${voiceMeter.advice}`}>{adviceText(voiceMeter)}</p>}
              <p class="practice-panel__hint">Sing or speak and watch which channel moves. The app never changes the input level: set it on your mixer so the bar sits in the marked band while you sing.</p>
            </fieldset>
          </div>
        )}
        <div class="practice-panel__actions">
          <button type="button" class="button button--primary" disabled={!dev || denied} onClick={() => void chooseInput(dev?.id ?? null, dev?.name ?? null, channel ?? suggested)}>
            Use this input
          </button>
          <button
            type="button"
            class="button"
            onClick={() => {
              void closePicker().then(() => watchOnly());
            }}
          >
            Watch only
          </button>
          <button type="button" class="button" onClick={() => void closePicker()}>
            Cancel
          </button>
        </div>
        {st?.input && (
          <p class="practice-panel__hint">
            Listening now: {st.input.device.name}, channel {st.input.channel + 1}
            {st.input.dropped_blocks > 0 ? ` (${st.input.dropped_blocks} blocks lost)` : ""}.
          </p>
        )}
      </div>
    </div>
  );
}
