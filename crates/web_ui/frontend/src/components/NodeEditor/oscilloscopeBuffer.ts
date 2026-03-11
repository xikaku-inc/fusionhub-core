const MAX_POINTS = 1800;

const tBuf = new Float64Array(MAX_POINTS);
const vBuf = new Float64Array(MAX_POINTS);
let len = 0;
let writePos = 0;
let gen = 0;
let lastT = -Infinity;

// Pre-allocated output arrays reused across getData calls
let outT: number[] = [];
let outV: number[] = [];

export function pushPoint(t: number, v: number) {
  // Reset on timestamp discontinuity (e.g. data source loop)
  if (t < lastT - 1) {
    len = 0;
    writePos = 0;
  }
  lastT = t;
  const pos = writePos % MAX_POINTS;
  tBuf[pos] = t;
  vBuf[pos] = v;
  writePos++;
  if (len < MAX_POINTS) len++;
  gen++;
}

export function getLength(): number {
  return len;
}

export function getGeneration(): number {
  return gen;
}

export function getData(): [number[], number[]] {
  if (len === 0) return [[], []];
  // Resize output arrays only when length changes
  if (outT.length !== len) {
    outT = new Array(len);
    outV = new Array(len);
  }
  const start = writePos >= MAX_POINTS ? writePos % MAX_POINTS : 0;
  for (let i = 0; i < len; i++) {
    const idx = (start + i) % MAX_POINTS;
    outT[i] = tBuf[idx];
    outV[i] = vBuf[idx];
  }
  return [outT, outV];
}

export function clearBuffer() {
  len = 0;
  writePos = 0;
  gen = 0;
  lastT = -Infinity;
  outT = [];
  outV = [];
}
