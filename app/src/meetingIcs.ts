import ICAL from "ical.js";

export function parseCalendar(text: string) {
  const calendar = new ICAL.Component(ICAL.parse(text.replace(/^\uFEFF/, "")));
  if (calendar.name !== "vcalendar") throw new Error("VCALENDAR がありません。");
  const zones = new Map(calendar.getAllSubcomponents("vtimezone").map(component => {
    const zone = new ICAL.Timezone(component);
    return [zone.tzid, zone] as const;
  }));
  const local = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const events = calendar.getAllSubcomponents("vevent");
  const entries = [];
  const skipped: string[] = [];
  function timestamp(time: ICAL.Time, tzid: string | undefined) {
    if (time.zone === ICAL.Timezone.utcTimezone) return time.toUnixTime() * 1000;
    if (tzid) {
      const zone = zones.get(tzid);
      if (zone) {
        time.zone = zone;
        return time.toUnixTime() * 1000;
      }
      // Resolve IANA zones even when the producer omits VTIMEZONE.
      const formatter = new Intl.DateTimeFormat("en-GB", {
        timeZone: tzid, year: "numeric", month: "numeric", day: "numeric",
        hour: "numeric", minute: "numeric", second: "numeric", hourCycle: "h23",
      });
      const wall = Date.UTC(time.year, time.month - 1, time.day, time.hour, time.minute, time.second);
      const display = (at: number) => {
        const parts = Object.fromEntries(formatter.formatToParts(at).map(part => [part.type, Number(part.value)]));
        return Date.UTC(parts.year, parts.month - 1, parts.day, parts.hour, parts.minute, parts.second);
      };
      const offsets = [wall - 86_400_000, wall, wall + 86_400_000].map(at => display(at) - at);
      const matches = offsets.map(offset => wall - offset).filter(at => display(at) === wall);
      if (!matches.length) throw new Error("TZID の現地時刻が存在しません。");
      return Math.min(...matches);
    }
    return new Date(time.year, time.month - 1, time.day, time.hour, time.minute, time.second).getTime();
  }

  for (const [index, component] of events.entries()) {
    try {
      const event = new ICAL.Event(component);
      if (["rrule", "rdate", "recurrence-id"].some(name => component.hasProperty(name))) {
        throw new Error("繰り返し予定・例外は対象外です。");
      }
      if (component.getFirstPropertyValue("status") === "CANCELLED") throw new Error("キャンセル済みです。");
      if (!event.uid?.trim() || !event.startDate) throw new Error("UID または開始時刻がありません。");
      const zoneId = (name: string) => {
        const value = component.getFirstProperty(name)?.getParameter("tzid");
        return typeof value === "string" ? value : undefined;
      };
      const startZone = zoneId("dtstart");
      const endZone = component.hasProperty("dtend") ? zoneId("dtend") : startZone;
      const start = timestamp(event.startDate, startZone);
      const end = timestamp(event.endDate, endZone);
      if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) throw new Error("終了時刻が開始時刻以下です。");
      const location = event.location || null;
      const joinUrl = [component.getFirstPropertyValue("conference"), component.getFirstPropertyValue("url"), location]
        .find((value): value is string => typeof value === "string" && /^https?:\/\/\S+$/i.test(value)) ?? null;
      entries.push({ id: crypto.randomUUID(), uid: event.uid, plan: {
        title: event.summary?.trim() || "件名なし", joinUrl, location, description: event.description || null,
        start, end, timeZone: startZone ?? (event.startDate.zone === ICAL.Timezone.utcTimezone ? "UTC" : local), agenda: [],
      } });
    } catch (error) {
      skipped.push(`予定 ${index + 1} · ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  if (!events.length) skipped.push("VEVENT がありません。");
  return { entries, skipped };
}
