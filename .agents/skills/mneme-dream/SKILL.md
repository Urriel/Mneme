---
name: mneme-dream
description: Turn newly ingested Mneme documents into atomic notes. Use when the user asks to dream, or at the end of a session. Do not run it on every ingest.
---

# Dream over Mneme

You are the only step that calls a language model. The `mneme` daemon stores documents, chunks, and notes. It does not summarize them.

Run this procedure once per dream. Stop if `list_recent` returns no rows.

1. Call `list_recent` with no `since`. The daemon applies the dream cursor.
2. For each new document, read `mneme://doc/{id}`. Use the full `text`.
3. Write one note per fact with `add_note`. Set `source_doc_id` to the document id. Set `links` to related note ids from this pass or from `search_note`.
4. When a new fact replaces an old one, set `supersedes` to the old note id. Do not delete the old note.
5. Call `set_cursor` with the newest `ingested_at` you processed.
6. Do not rewrite a document row. Do not call `ingest` on text the daemon already stored.

`search_doc` and `search_note` return an excerpt. `get` returns the full row. The same row is at `mneme://doc/{id}`.
