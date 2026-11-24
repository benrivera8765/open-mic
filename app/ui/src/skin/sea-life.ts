// SPDX-License-Identifier: GPL-3.0-or-later
// Fish shared by the aquarium cabinet (Aquarium.tsx) and the ocean-tube border
// (OceanTube.tsx), so the two read as one set: clownfish, blue and yellow tangs,
// angelfish and neon tetras, drawn with plain canvas paths.

export type FishKind = "clown" | "blueTang" | "yellowTang" | "angel" | "tetra";

function eye(ctx: CanvasRenderingContext2D, x: number, y: number, r: number): void {
  ctx.fillStyle = "#ffffff";
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = "#101010";
  ctx.beginPath();
  ctx.arc(x + r * 0.2, y, r * 0.55, 0, Math.PI * 2);
  ctx.fill();
}

/** A fish in a 30-unit box facing right, centred on the origin; tail swings by `w` (radians). */
export function drawFishShape(ctx: CanvasRenderingContext2D, kind: FishKind, w: number): void {
  const tail = (len: number, spread: number, color: string) => {
    ctx.save();
    ctx.translate(-12, 0);
    ctx.rotate(w);
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(1, 0);
    ctx.lineTo(-len, -spread);
    ctx.quadraticCurveTo(-len * 0.7, 0, -len, spread);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
  };
  if (kind === "clown") {
    tail(8, 6, "#ff7a1a");
    const g = ctx.createLinearGradient(0, -8, 0, 8);
    g.addColorStop(0, "#ff9a3a");
    g.addColorStop(1, "#e85a10");
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.ellipse(0, 0, 15, 8, 0, 0, Math.PI * 2);
    ctx.fill();
    // White bands edged in black, kept inside the body.
    ctx.save();
    ctx.clip();
    for (const bx of [7, -1, -9]) {
      ctx.fillStyle = "#1a1a1a";
      ctx.fillRect(bx - 2.2, -9, 4.4, 18);
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(bx - 1.5, -9, 3, 18);
    }
    ctx.restore();
    eye(ctx, 10, -2, 2);
  } else if (kind === "blueTang") {
    tail(9, 7, "#ffd21a");
    ctx.fillStyle = "#1f5fe0";
    ctx.beginPath();
    ctx.ellipse(0, 0, 15, 9, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = "#0a1030";
    ctx.lineWidth = 2.6;
    ctx.beginPath();
    ctx.moveTo(10, -4);
    ctx.quadraticCurveTo(-2, -8, -11, -1);
    ctx.quadraticCurveTo(-2, 2, 6, 0);
    ctx.stroke();
    ctx.fillStyle = "#3b7cff";
    ctx.beginPath();
    ctx.moveTo(-6, -8);
    ctx.quadraticCurveTo(2, -13, 10, -7);
    ctx.closePath();
    ctx.fill();
    eye(ctx, 10, -2, 2);
  } else if (kind === "yellowTang") {
    tail(7, 6, "#ffcc10");
    ctx.fillStyle = "#ffd21a";
    ctx.beginPath();
    ctx.ellipse(0, 0, 12, 11, 0, 0, Math.PI * 2);
    ctx.moveTo(10, -3);
    ctx.lineTo(16, 0);
    ctx.lineTo(10, 3);
    ctx.fill();
    ctx.fillStyle = "#ffe46a";
    ctx.beginPath();
    ctx.moveTo(-8, -9);
    ctx.quadraticCurveTo(0, -16, 8, -9);
    ctx.closePath();
    ctx.moveTo(-8, 9);
    ctx.quadraticCurveTo(0, 16, 8, 9);
    ctx.closePath();
    ctx.fill();
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(-11, -1, 3, 2);
    eye(ctx, 8, -3, 1.9);
  } else if (kind === "angel") {
    tail(9, 7, "rgba(230,230,240,0.9)");
    ctx.fillStyle = "rgba(235,235,245,0.95)";
    ctx.beginPath();
    ctx.moveTo(-6, -8);
    ctx.lineTo(-14, -26);
    ctx.lineTo(4, -9);
    ctx.closePath();
    ctx.moveTo(-6, 8);
    ctx.lineTo(-14, 26);
    ctx.lineTo(4, 9);
    ctx.closePath();
    ctx.fill();
    ctx.beginPath();
    ctx.ellipse(0, 0, 11, 10, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#1a1a24";
    for (const bx of [6, -1, -7]) ctx.fillRect(bx - 1.3, -10, 2.6, 20);
    eye(ctx, 8, -2, 1.8);
  } else {
    tail(6, 4, "#ff3a3a");
    ctx.fillStyle = "#d8e0e8";
    ctx.beginPath();
    ctx.ellipse(0, 0, 14, 5, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#ff3a3a";
    ctx.beginPath();
    ctx.ellipse(-5, 2, 8, 2.6, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = "#29d8ff";
    ctx.fillRect(-10, -2, 20, 2.4);
    eye(ctx, 9, -1, 1.6);
  }
}
