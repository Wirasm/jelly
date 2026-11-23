// What the relay counts about its own use, in daily buckets (UTC): shares made,
// shares by someone who had shared before, links opened, and how many people
// each share had connected at once. It keeps no IP, no email and nothing from a
// jelly: only numbers, share ids, and a hash of the publish secret.
//
// The tables live in the relay's one `Pins` object, so counting needs no
// binding or migration of its own.

/** How many days GET /api/counts lists. `total` covers every day counted. */
const DAYS = 30;

export const today = (ms = Date.now()) => new Date(ms).toISOString().slice(0, 10);

export function tables(sql: SqlStorage) {
  sql.exec(`CREATE TABLE IF NOT EXISTS days (day TEXT PRIMARY KEY, shares INTEGER NOT NULL DEFAULT 0, returning_shares INTEGER NOT NULL DEFAULT 0, opens INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS sharers (id TEXT PRIMARY KEY);
    CREATE TABLE IF NOT EXISTS rooms (day TEXT NOT NULL, share TEXT NOT NULL, peak INTEGER NOT NULL, PRIMARY KEY (day, share));`);
}

/** A jelly was shared. `sharer` is a hash of the publish secret that did it:
 *  a share is a returning one when that secret has shared before. */
export function shared(sql: SqlStorage, sharer: string) {
  const returning = sql.exec("INSERT INTO sharers (id) VALUES (?) ON CONFLICT(id) DO NOTHING", sharer).rowsWritten === 0;
  sql.exec("INSERT INTO days (day, shares, returning_shares) VALUES (?, 1, ?) ON CONFLICT(day) DO UPDATE SET shares = shares + 1, returning_shares = returning_shares + excluded.returning_shares",
    today(), returning ? 1 : 0);
}

/** Someone's page connected to `share`, which now has `people` connected.
 *  `first` when it is their link's first time today: that is one open. */
export function opened(sql: SqlStorage, share: string, people: number, first: boolean) {
  const day = today();
  sql.exec("INSERT INTO rooms (day, share, peak) VALUES (?, ?, ?) ON CONFLICT(day, share) DO UPDATE SET peak = MAX(peak, excluded.peak)", day, share, people);
  if (first) sql.exec("INSERT INTO days (day, opens) VALUES (?, 1) ON CONFLICT(day) DO UPDATE SET opens = opens + 1", day);
}

interface Day { shares: number; returning_shares: number; opens: number; rooms: Record<string, number> }

/** The last DAYS days that had anything, oldest first, and the sum of every day.
 *  `rooms` maps a number of people to how many shares peaked at it that day,
 *  so "rooms of 3 or more" is the sum of the keys from 3 up. */
export function counts(sql: SqlStorage) {
  const days = new Map<string, Day>();
  const total: Day & { sharers: number } = { shares: 0, returning_shares: 0, opens: 0, rooms: {}, sharers: sql.exec<{ n: number }>("SELECT COUNT(*) AS n FROM sharers").one().n };
  const at = (day: string) => days.get(day) ?? days.set(day, { shares: 0, returning_shares: 0, opens: 0, rooms: {} }).get(day)!;
  for (const { day, ...n } of sql.exec<{ day: string; shares: number; returning_shares: number; opens: number }>("SELECT day, shares, returning_shares, opens FROM days")) {
    Object.assign(at(day), n);
    total.shares += n.shares; total.returning_shares += n.returning_shares; total.opens += n.opens;
  }
  for (const { day, peak, n } of sql.exec<{ day: string; peak: number; n: number }>("SELECT day, peak, COUNT(*) AS n FROM rooms GROUP BY day, peak ORDER BY peak")) {
    at(day).rooms[peak] = n;
    total.rooms[peak] = (total.rooms[peak] ?? 0) + n;
  }
  return { days: [...days].sort(([a], [b]) => a.localeCompare(b)).slice(-DAYS).map(([day, n]) => ({ day, ...n })), total };
}
