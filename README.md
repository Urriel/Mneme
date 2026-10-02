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

Quit this process before another client opens the same directory. One process owns a data directory.

Pass `--model`, `--dim`, and `--device cpu` to both commands when you do not want the default model. The default dimension is 1024. Opening a directory with a different model or dimension fails. `mneme reembed` prints the locked model and does not rewrite vectors.

## Tell an agent about Mneme

`mneme setup` installs the skills into the current project and adds one pointer to `AGENTS.md`. `CLAUDE.md` points at that file when it is missing. `mneme setup --global` installs the same skills under your home directory and adds the pointer to `~/.claude/CLAUDE.md`. A second run leaves an edited skill in place. The command prints the MCP server block and does not write it.

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

`search` takes `query` and an optional `k` from 1 to 40. The result is `{"hits":[{"doc_id","title","path","score","excerpt"}]}`. The excerpt is at most 400 characters. Read `mneme://doc/{id}` for the full text. Optional `as_of` is a Lance table version. Optional `include_superseded` keeps replaced documents in the hits.

`list_recent` lists documents and notes ingested after `since`. With no `since`, the daemon uses the dream cursor.

`add_note` stores one note. It does not change the source document.

`link` appends one note id to another note.

`set_cursor` advances the dream cursor. The dream skill is the caller.

## Dream

An agent that calls the daemon follows `.agents/skills/mneme/SKILL.md`. That skill chooses ingest, search, and a full-document read. The dream procedure is `.agents/skills/mneme-dream/SKILL.md`. Claude Code reads the same files through `.claude/skills/`. The daemon does not call a language model.

## Search in this version

Search embeds the query, takes the nearest 40 chunks, takes the 40 best BM25 chunk matches, and fuses the two lists. It returns documents, not chunks. There is no separate ANN index.
