# Claude Session History Pagination Design

**Status:** approved for implementation  
**Date:** 2026-09-04

## Goal

The Claude session loader must let a user browse every recoverable session and every event in a
selected session, search the complete transcript, and jump to any result without materializing the
complete history in the renderer.

## User-visible behavior

1. The left session list has no total-result cap. It loads the newest page first and continues as the
   user scrolls. Its compact status reports loaded and total counts.
2. The right preview opens on the newest event page. Scrolling near the top loads earlier events and
   preserves the visible reading anchor.
3. The existing “latest 240 events” limitation notice is removed. A compact status reports the loaded
   window and full event count only when more history exists.
4. Right-side search covers the complete selected transcript. The match counter is the complete result
   count, and previous/next navigation can reach every match.
5. Navigating to a search result loads a bounded context window around the stable event index and
   centers the matched event.
6. Loading a selected Claude session keeps the existing duplicate-session warning, running-card
   confirmation, provider binding, model, and permission behavior.

## Architecture

### Runtime catalog

- Replace cached full event objects with a per-file lightweight index containing file signature,
  transcript metadata, event byte ranges, and event display metadata.
- Build the index with a bounded-memory JSONL scanner. Reuse it while `(mtimeMs, size)` remains stable.
- Read only the contiguous byte range needed for an event page and parse only events in that page.
- Search the complete indexed transcript in bounded byte batches. Cache lightweight hit lists by file
  signature and normalized query.
- Keep event indexes stable and one-based so search hits and event pages share one navigation key.

### RPC contracts

- `claude-sessions.list`: accepts `offset` and `limit`, returns `offset`, `limit`, `nextOffset`, and
  `hasMore` in addition to `sessions` and `total`.
- `claude-sessions.detail`: accepts `beforeEventIndex`, `aroundEventIndex`, and `limit`; returns a
  bounded event page plus `page` metadata.
- `claude-sessions.search`: accepts `query`, `offset`, and `limit`; returns the exact total and a page of
  lightweight hits.

Limits are request batch sizes, not history visibility limits.

### Renderer

- Keep independent left metadata filtering and right transcript search.
- Page and virtualize the left results and right event list.
- Serialize prepend requests and ignore stale responses after session/query changes.
- Restore the visible event anchor after prepending older events.
- When search navigation points outside the current event window, request an around-event page and
  focus the result after it renders.

## Performance and correctness gates

- A 24,000-event transcript reports the exact total while the first detail response contains at most
  200 events.
- A query matching beyond the initial page is returned and can be opened with surrounding context.
- Renderer DOM row counts stay bounded by viewport size plus overscan.
- Repeated detail pages reuse the same file index while the source signature is unchanged.
- Appending to a transcript invalidates stale page/search data and exposes the appended event.
- Left and right pagination preserve selection, focus, scroll position, duplicate warnings, and load
  actions.

## Non-goals

- Changing Claude Code transcript files.
- Persisting a second full copy of transcript content.
- Changing provider resume, permission, model, or duplicate-load semantics.
