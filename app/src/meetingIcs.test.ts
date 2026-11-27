import assert from "node:assert/strict";
import test from "node:test";
import { parseCalendar } from "./meetingIcs.ts";

test("imports multiple UTC, TZID and all-day events and reports recurring events", () => {
  const text = [
    "BEGIN:VCALENDAR", "VERSION:2.0",
    "BEGIN:VEVENT", "UID:tokyo", "SUMMARY:Tokyo", "LOCATION:Room 2", "DESCRIPTION:First\\nSecond",
    "DTSTART;TZID=Asia/Tokyo:20261128T090000", "DTEND;TZID=Asia/Tokyo:20261128T100000", "END:VEVENT",
    "BEGIN:VEVENT", "UID:utc", "SUMMARY:UTC", "URL:https://example.com/join",
    "DTSTART:20261128T040000Z", "DTEND:20261128T050000Z", "END:VEVENT",
    "BEGIN:VEVENT", "UID:date", "SUMMARY:All day", "DTSTART;VALUE=DATE:20261128", "END:VEVENT",
    "BEGIN:VEVENT", "UID:series", "DTSTART:20261128T040000Z", "RRULE:FREQ=WEEKLY", "END:VEVENT",
    "END:VCALENDAR",
  ].join("\r\n");
  const result = parseCalendar(text);
  assert.deepEqual(result.entries.map(item => item.uid), ["tokyo", "utc", "date"]);
  assert.equal(result.entries[0].plan.start, Date.parse("2026-11-28T00:00:00Z"));
  assert.equal(result.entries[0].plan.end, Date.parse("2026-11-28T01:00:00Z"));
  assert.equal(result.entries[0].plan.location, "Room 2");
  assert.equal(result.entries[0].plan.description, "First\nSecond");
  assert.equal(result.entries[1].plan.start, Date.parse("2026-11-28T04:00:00Z"));
  assert.equal(result.entries[1].plan.joinUrl, "https://example.com/join");
  assert.equal(result.entries[2].plan.start, new Date(2026, 10, 28).getTime());
  assert.equal(result.entries[2].plan.end, new Date(2026, 10, 29).getTime());
  assert.equal(result.skipped.length, 1);
});

test("uses embedded VTIMEZONE for non-IANA calendar identifiers", () => {
  const calendar = [
    "BEGIN:VCALENDAR", "VERSION:2.0",
    "BEGIN:VTIMEZONE", "TZID:Example Standard Time",
    "BEGIN:STANDARD", "DTSTART:19700101T000000", "TZOFFSETFROM:+0530", "TZOFFSETTO:+0530",
    "END:STANDARD", "END:VTIMEZONE",
    "BEGIN:VEVENT", "UID:custom", "SUMMARY:Custom zone",
    "DTSTART;TZID=Example Standard Time:20261128T090000",
    "DTEND;TZID=Example Standard Time:20261128T100000",
    "END:VEVENT", "END:VCALENDAR",
  ].join("\r\n");
  const result = parseCalendar(calendar);
  assert.deepEqual(result.skipped, []);
  assert.equal(result.entries[0].plan.start, Date.parse("2026-11-28T03:30:00Z"));
  assert.equal(result.entries[0].plan.end, Date.parse("2026-11-28T04:30:00Z"));
});
