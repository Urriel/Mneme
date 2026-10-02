# Mneme

Mneme is one local process that stores documents and searches them by meaning and by words. You run the `mneme` binary. It speaks MCP on stdin and stdout. Embeddings run on this machine. The files stay in the directory you pass to `--data`.

The default directory is `./mneme-data`. A second `--data` path is a second brain.

## Create a brain

Install `protoc` before the first compile. On macOS, run `brew install protobuf`.

```bash
mneme init --data brains/personal
mneme run --data brains/personal
```

`init` creates the folder and locks the embedding model. `run` serves MCP. The first `run` downloads `Qwen/Qwen3-Embedding-0.6B` into `.fastembed_cache` in the current directory. Set `FASTEMBED_CACHE_DIR` to store that cache somewhere else. The cache is not inside the `--data` directory.

The process logs to stderr. Stdout is the MCP protocol.

One process owns a data directory. A later `mneme run` for that same directory attaches to the owner and speaks MCP on its own stdin and stdout. When the owner exits, the attached clients close.

Pass `--model`, `--dim`, and `--device cpu` to both commands when you do not want the default model. The default dimension is 1024. Opening a directory with a different model or dimension fails. `mneme reembed` prints the locked model and does not rewrite vectors.

## Tell an agent about Mneme

`mneme setup` installs the skills into the current project and adds one pointer to `AGENTS.md`. `CLAUDE.md` points at that file when it is missing. `mneme setup --global` installs the same skills under your home directory and adds the pointer to `~/.claude/CLAUDE.md`. A second run leaves an edited skill in place.

The command also looks for harness config already on this machine. Cursor gets `.cursor/mcp.json` or `~/.cursor/mcp.json`. Claude Code gets `.mcp.json` or `~/.claude.json`. Claude Desktop gets `claude_desktop_config.json`. Grok gets `[mcp_servers.mneme]` in `.grok/config.toml` or `~/.grok/config.toml`. An existing `mneme` entry with different arguments is left as it is. When none of those directories exist, the command prints the server block instead.

```bash
mneme setup --data brains/personal
mneme setup --global --data brains/personal
```

## Connect Claude Desktop

```json
{
  "mcpServers": {
    "mneme": {
      "command": "/absolute/path/to/mneme",
      "args": ["run", "--data", "/absolute/path/brains/personal"]
    }
  }
}
```

Replace both paths with paths on your machine.

## Tools

`ingest` stores a document from `path` or from `text`. The result is `{"doc_id":"<ulid>"}`. The same bytes return the same id. A changed file writes a new row and sets `supersedes` to the previous id.

`search_doc` takes `query` and an optional `k` from 1 to 40. The result is `{"hits":[{"doc_id","title","path","score","excerpt"}]}`. The excerpt is at most 400 characters. Call `get` or read `mneme://doc/{id}` for the full text. Optional `as_of` is a Lance table version. Optional `include_superseded` keeps replaced documents in the hits.

`search_note` takes `query` and an optional `k` from 1 to 40. The result is `{"hits":[{"id","source_doc_id","score","excerpt"}]}`. It searches notes. A replaced note stays out of the hits. Call `get` or read `mneme://doc/{id}` for the full note.

`get` takes `id`. The result is the full row for that document or note. The fields are `id`, `kind`, `text`, `supersedes`, `links`, `title`, `path`, `doc_id`, and `source_doc_id`. The same row is at `mneme://doc/{id}`.

`list_recent` lists documents and notes ingested after `since`. With no `since`, the daemon uses the dream cursor.

`add_note` stores one note. It does not change the source document.

`link` appends one note id to another note.

`set_cursor` advances the dream cursor. The dream skill is the caller.

## Dream

An agent that calls the daemon follows `.agents/skills/mneme/SKILL.md`. That skill chooses ingest, `search_doc`, `search_note`, and `get`. The dream procedure is `.agents/skills/mneme-dream/SKILL.md`. Claude Code reads the same files through `.claude/skills/`. The daemon does not call a language model.

## Search in this version

`search_doc` embeds the query, takes the nearest 40 chunks, takes the 40 best BM25 chunk matches, and fuses the two lists. It returns documents, not chunks. `search_note` fuses the nearest notes with the best BM25 note matches. There is no separate ANN index.
