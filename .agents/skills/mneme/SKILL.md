---
name: mneme
description: >
  Decide when to call the Mneme MCP and which tool to use.
  Use when the user says remember this, search my notes, what did I store,
  ingest this file, or when the tools ingest, search_doc, search_note, get,
  list_recent, add_note, link, or set_cursor are available.
---

# Use the Mneme MCP

Mneme is the local document store. You call it. It does not call a model.

## When to call it

Search documents with `search_doc` before you answer about a person, a project, or a file the user asked you to keep. Search notes with `search_note`.

Ingest when the user gives a path or a text and asks you to keep it. Ingest that text once.

Call `get` with the id when you need more than the search excerpt. The same row is at `mneme://doc/{id}`.

Dream when the user asks, or at the end of a session. Follow `.agents/skills/mneme-dream/SKILL.md`. Do not dream on an ingest.

## Why the calls are split

A search hit is an id plus an excerpt of at most 400 characters. The chunk exists so `search_doc` can find the document. When the words matter, quote the `text` from `get`.

The same bytes return the same document id. Call `ingest` again to learn that the text is already stored.

A changed file becomes a new document. The new row sets `supersedes` to the old id. Leave the old document text as it is.

Notes, links, and `set_cursor` belong to the dream pass. A chat answer is not a note. The daemon does not summarize.

## How to call it

1. Call `search_doc` with the user's question about a document. Call `search_note` when the question is about a note. `k` defaults to 5. Keep it in 1 to 40.
2. When a hit is the right row, call `get` before you rely on the full text. A document hit uses `doc_id`. A note hit uses `id`. The same row is at `mneme://doc/{id}`.
3. To keep a file, call `ingest` with `path`. To keep pasted text, call `ingest` with `text`. Pass `title` when you have one.
4. Leave `add_note`, `link`, and `set_cursor` to the dream skill.
5. Pass `include_superseded` only when the user wants the replaced document. Pass `as_of` only when the user names a Lance version. That search uses vectors only.

One process owns the folder. A later `mneme run` for that folder attaches to the owner. A second folder is a second brain. Point `--data` at a new folder. A folder that still has `brain.toml` is the older notes layout.
