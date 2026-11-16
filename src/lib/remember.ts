// Per-viewer UI memory (last area, last tab). Storage may be unavailable; the app works without it.

export function remembered<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(`ui.${key}`);
    return allowed.includes(v as T) ? (v as T) : fallback;
  } catch {
    return fallback;
  }
}

export function remember(key: string, value: string) {
  try {
    localStorage.setItem(`ui.${key}`, value);
  } catch {
    // not remembered; nothing else depends on it
  }
}
