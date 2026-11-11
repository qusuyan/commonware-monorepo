#!/usr/bin/env node
// Render the two primary MMR comparisons from the normalized Criterion summary.
// Usage: node render-selected-graphs.mjs <summary.json> <output-directory>

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const [summaryPath, outputDir] = process.argv.slice(2);
if (!summaryPath || !outputDir) {
  throw new Error("usage: render-selected-graphs.mjs <summary.json> <output-directory>");
}

const rows = JSON.parse(await readFile(summaryPath, "utf8"));
const required = {
  index_order: "unordered",
  value_layout: "fixed",
  merkle_family: "mmr",
  keyspace_keys: "10000",
  batch_parent: "directly from database",
  seed_synced: "yes",
  commonware_page_cache: "retained (warm)",
};

function selected(authMode, chunk) {
  const row = rows.find((candidate) =>
    Object.entries(required).every(([key, value]) => candidate[key] === value) &&
    candidate.auth_mode === authMode &&
    candidate.bitmap_chunk_bytes === chunk,
  );
  if (!row) throw new Error(`missing selected row: ${authMode}, chunk=${chunk || "none"}`);
  return row;
}

const any = selected("any", "");
const current32 = selected("current", "32");
const current256 = selected("current", "256");

const escape = (value) => String(value)
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;");

async function chart({ file, title, comparison, entries }) {
  const width = 1200;
  const height = 720;
  const plotLeft = 365;
  const plotRight = 1100;
  const plotTop = 245;
  const plotBottom = 575;
  const lower = Math.min(...entries.map((entry) => entry.mean_ci_low_us));
  const upper = Math.max(...entries.map((entry) => entry.mean_ci_high_us));
  const span = upper - lower;
  const min = Math.floor((lower - span * 0.16) / 20) * 20;
  const max = Math.ceil((upper + span * 0.16) / 20) * 20;
  const x = (value) => plotLeft + ((value - min) / (max - min)) * (plotRight - plotLeft);
  const ticks = Array.from({ length: 6 }, (_, index) => min + ((max - min) * index) / 5);
  const ys = entries.length === 2 ? [360, 480] : [];
  const delta = ((entries[1].mean_us / entries[0].mean_us) - 1) * 100;
  const deltaText = `${delta >= 0 ? "+" : ""}${delta.toFixed(1)}%`;
  const color = ["#64748b", "#1976b9"];

  const grid = ticks.map((tick) => `
    <line x1="${x(tick)}" y1="${plotTop}" x2="${x(tick)}" y2="${plotBottom}" stroke="#d7dde3" stroke-width="1"/>
    <text x="${x(tick)}" y="${plotBottom + 36}" text-anchor="middle" class="tick">${tick.toFixed(0)}</text>`).join("");
  const marks = entries.map((entry, index) => {
    const y = ys[index];
    const fill = color[index];
    return `
      <text x="${plotLeft - 28}" y="${y - 10}" text-anchor="end" class="case">${escape(entry.label)}</text>
      <text x="${plotLeft - 28}" y="${y + 18}" text-anchor="end" class="detail">${escape(entry.detail)}</text>
      <line x1="${x(entry.mean_ci_low_us)}" y1="${y}" x2="${x(entry.mean_ci_high_us)}" y2="${y}" stroke="${fill}" stroke-width="7" stroke-linecap="round" opacity="0.55"/>
      <line x1="${x(entry.mean_ci_low_us)}" y1="${y - 15}" x2="${x(entry.mean_ci_low_us)}" y2="${y + 15}" stroke="${fill}" stroke-width="3"/>
      <line x1="${x(entry.mean_ci_high_us)}" y1="${y - 15}" x2="${x(entry.mean_ci_high_us)}" y2="${y + 15}" stroke="${fill}" stroke-width="3"/>
      <circle cx="${x(entry.mean_us)}" cy="${y}" r="10" fill="${fill}"/>
      <text x="${Math.min(x(entry.mean_us) + 18, plotRight - 5)}" y="${y + 6}" class="value">${entry.mean_us.toFixed(1)} us</text>`;
  }).join("");

  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-labelledby="title description">
  <title id="title">${escape(title)}</title>
  <desc id="description">${escape(comparison)}. Dots show Criterion mean time and horizontal bars show the 95 percent bootstrap confidence interval.</desc>
  <rect width="100%" height="100%" fill="white"/>
  <style>
    text { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; fill: #18212b; }
    .title { font-size: 28px; font-weight: 650; }
    .subtitle { font-size: 17px; fill: #4b5968; }
    .case { font-size: 20px; font-weight: 650; }
    .detail { font-size: 15px; fill: #4b5968; }
    .tick { font-size: 16px; fill: #4b5968; }
    .axis { font-size: 18px; font-weight: 600; }
    .value { font-size: 19px; font-weight: 650; }
    .note { font-size: 16px; fill: #4b5968; }
  </style>
  <text x="70" y="65" class="title">${escape(title)}</text>
  <text x="70" y="98" class="subtitle">MMR · unordered index · fixed 32-byte values · 10,000 seeded keys · direct batch (ch=false)</text>
  <text x="70" y="126" class="subtitle">Seed synced · Commonware page cache retained (warm) · Criterion mean with 95% bootstrap confidence interval</text>
  <text x="70" y="165" class="subtitle">${escape(comparison)}</text>
  <text x="70" y="199" class="subtitle">Result: second case is ${deltaText} relative to first case (${(entries[1].mean_us - entries[0].mean_us).toFixed(1)} us).</text>
  ${grid}
  <line x1="${plotLeft}" y1="${plotBottom}" x2="${plotRight}" y2="${plotBottom}" stroke="#6b7785" stroke-width="2"/>
  ${marks}
  <text x="${(plotLeft + plotRight) / 2}" y="${height - 58}" text-anchor="middle" class="axis">Mean time per timed iteration (microseconds)</text>
  <text x="70" y="${height - 22}" class="note">Dot: Criterion mean. Horizontal bar: 95% bootstrap confidence interval. Each configuration: 10 Criterion samples. This is one partial benchmark process run.</text>
</svg>`;

  await writeFile(join(outputDir, file), svg);
}

await mkdir(outputDir, { recursive: true });

await chart({
  file: "mmr-any-vs-current-chunk-32.svg",
  title: "MMR baseline: any versus current with 32-byte bitmap chunks",
  comparison: "Cases compared: any (local, unauthenticated bitmap) versus current (bitmap authenticated through 32-byte grafted chunks).",
  entries: [
    { ...any, label: "any", detail: "local bitmap; not included in authenticated root" },
    { ...current32, label: "current, chunk=32", detail: "bitmap chunks grafted into authenticated root" },
  ],
});

await chart({
  file: "mmr-current-chunk-32-vs-256.svg",
  title: "MMR chunk-size sensitivity: current 32-byte versus 256-byte chunks",
  comparison: "Cases compared: current with 32-byte bitmap chunks versus current with 256-byte bitmap chunks.",
  entries: [
    { ...current32, label: "current, chunk=32", detail: "32-byte bitmap chunks" },
    { ...current256, label: "current, chunk=256", detail: "256-byte bitmap chunks" },
  ],
});
