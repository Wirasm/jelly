// Which stored events of a share go. The owner's daemon says which ones it has
// on disk (`{"saved": seq}`); of those, the relay deletes the ones older than
// a week by its own clock, and never the newest, which its seq counter goes on
// from when the object wakes. A daemon away for less than a week (a laptop
// that slept, a home restored from a recent backup, a second machine) then
// finds every event it missed and hears each one; one away longer catches up
// from the whole jelly. relay/test/trim.test.mjs runs this against SQLite.

/** How long the relay keeps an event the daemon has saved. */
export const KEEP_MS = 7 * 86_400_000;

/** The part of `SqlStorage` this needs, so a test can hand it another SQLite. */
export interface Sql { exec(query: string, ...bindings: (string | number)[]): unknown }

/** Deletes the events with seq up to `saved`, below `newest`, made before `now - KEEP_MS`.
 *  `at_ms` is what the relay stamped on the event when it made it (`ops.apply`). */
export function forget(sql: Sql, saved: number, newest: number, now: number) {
  sql.exec("DELETE FROM events WHERE seq <= ? AND seq < ? AND json_extract(ev, '$.at_ms') < ?", saved, newest, now - KEEP_MS);
}
