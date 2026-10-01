# Mneme

Mneme is one local process that stores notes and searches them by meaning. You run the `mneme` binary. It speaks MCP on stdin and stdout. Embeddings run on this machine. The notes stay in the directory you pass to `--data`.

## Create a brain

Install `protoc` before the first compile. On macOS, run `brew install protobuf`.

```bash
mkdir -p brains/personal
cargo run --release -- --data brains/personal
```

The first start downloads the embedding model into `.fastembed_cache` in the current directory. Set `FASTEMBED_CACHE_DIR` to store that cache somewhere else. The cache is not inside the `--data` directory.

The process logs to stderr. Stdout is the MCP protocol.

Quit this process before Claude Desktop starts the same directory. One process owns a data directory.

The release binary is `target/release/mneme`.

## Connect Claude Desktop

```json
{
  "mcpServers": {
    "mneme": {
      "command": "/absolute/path/to/mneme",
      "args": ["--data", "/absolute/path/brains/personal"]
    }
  }
}
```

Replace both paths with paths on your machine.

## Tools

`memory_add` stores one note. Pass `text` and an optional `metadata` object. The result is `{"id":"<uuid>"}`.

`memory_search` takes `query` and an optional `limit`. The result is `{"hits":[{"id","text","score","metadata"}]}`. A higher `score` means a closer note. `limit` defaults to 5. `memory_search` rejects a limit outside 1 to 100.

## A second brain

A second `--data` path is a separate brain. Notes in `brains/company` stay out of `brains/personal`.

## Search in this version

Search compares the query with every stored vector. This version does not build an ANN index.
