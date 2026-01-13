# Claude History Cleaner (chc)

A CLI tool to manage and clean Claude Code conversation history stored in `~/.claude/projects/`.

## Features

- Table-style interface with LAST ACTIVE, TITLE, PROJECT columns
- **Shows last active time** (not start time) for each conversation
- **Smooth scrolling** - viewport follows cursor, adapts to terminal height
- **Active conversation detection** - warns before deleting conversations modified in last 5 minutes
- **Smart sorting** - conversations with content first, empty ones at the end
- Keyboard navigation (j/k or arrow keys, PageUp/PageDown)
- Multi-select with space key
- Auto-detection and cleanup of empty/warmup conversations
- **Always shows list before deletion** with confirmation prompt
- **Error reporting** when deletion fails
- Filter by workspace
- Excludes agent/subagent files by default (use `--include-agents` to show)

## What Gets Cleaned

| Type | Description | Default Behavior |
|------|-------------|------------------|
| Empty | 0-byte files (session started but never used) | Shown, can batch delete |
| Warmup | Agent warmup messages (internal optimization) | Hidden by default |
| Agent | Subagent conversation logs (`agent-*.jsonl`) | Hidden by default |

## Claude Code Directory Structure

Claude Code stores conversation history in `~/.claude/projects/`. Here's the complete structure:

```
~/.claude/
├── history.jsonl                    # Index of all conversations
├── settings.json                    # Global settings (including cleanupPeriodDays)
├── .credentials.json                # Authentication
└── projects/
    ├── -home-user-myproject/        # Workspace folder (path encoded: / → -)
    │   ├── abc123-def456.jsonl      # Main conversation (JSONL format)
    │   ├── abc123-def456/           # Related files for this conversation
    │   │   └── subagents/           # Subagent transcripts (new format)
    │   │       └── agent-a1b2c3.jsonl
    │   └── agent-xyz789.jsonl       # Subagent conversation (old format, legacy)
    └── -home-user-another/
        └── ...
```

### Path Encoding

Claude encodes workspace paths by replacing `/` with `-`:
- `/home/user/myproject` → `-home-user-myproject`

### Conversation Files (.jsonl)

Each conversation is stored in [JSON Lines](https://jsonlines.org/) format. Each line is a JSON object with:
- `type`: Message type (`user`, `assistant`, `system`)
- `message`: Content object
- `timestamp`: ISO 8601 timestamp

### Subagent Storage (Version Change)

Claude Code changed how subagent files are stored:

| Version | Location | Example |
|---------|----------|---------|
| Old | Workspace root | `-home-user-project/agent-abc123.jsonl` |
| New | Inside conversation folder | `-home-user-project/{sessionId}/subagents/agent-abc123.jsonl` |

When you delete a main conversation, `chc` will:
1. Delete the `.jsonl` file
2. Delete the related folder (including `subagents/`)
3. Delete legacy `agent-*.jsonl` files that reference this conversation

### Auto Cleanup

By default, Claude Code deletes conversation files after 30 days. You can change this in `~/.claude/settings.json`:
```json
{
  "cleanupPeriodDays": 99999
}
```

### References

- [Claude Code's hidden conversation history](https://kentgigger.com/posts/claude-code-conversation-history)
- [Create custom subagents - Claude Code Docs](https://code.claude.com/docs/en/sub-agents)

## Installation

### Pre-built Binaries (Recommended)

Download from [Releases](https://github.com/mq-loser/claude-history-cleaner/releases):

| Platform | Architecture | File |
|----------|-------------|------|
| Linux | x86_64 | `chc-linux-x86_64` |
| Linux | aarch64 | `chc-linux-aarch64` |
| macOS | Intel | `chc-macos-x86_64` |
| macOS | Apple Silicon | `chc-macos-aarch64` |
| Windows | x86_64 | `chc-windows-x86_64.exe` |

Linux binaries are statically linked (musl) and work on any distribution (Ubuntu, Debian, Fedora, Arch, etc.).

```bash
# Example for Linux x86_64
curl -L https://github.com/mq-loser/claude-history-cleaner/releases/latest/download/chc-linux-x86_64 -o chc
chmod +x chc
sudo mv chc /usr/local/bin/
```

### Build from Source

Requires [Rust](https://rustup.rs/).

```bash
git clone https://github.com/mq-loser/claude-history-cleaner.git
cd claude-history-cleaner
cargo build --release
cargo install --path .
```

## Usage

```bash
# Interactive mode (default)
chc

# List all workspaces
chc -l

# Filter by workspace
chc -w myproject

# Include agent/warmup conversations
chc --include-agents

# Delete all empty (will show list and ask for confirmation)
chc --delete-empty

# Delete both empty and warmup
chc --delete-empty --delete-warmup
```

## Controls

| Key | Action |
|-----|--------|
| j/k or ↑/↓ | Move cursor |
| Space | Toggle selection |
| a | Select all |
| n | Deselect all |
| Enter | Confirm deletion (when items selected) |
| q/Esc | Quit |

## Screenshot

```
Claude Code Chat Manager
Total: 42 | Selected: 0 | Showing: 1-15/42
  1 active (modified <5min, marked with *)

    LAST ACTIVE          TITLE                                              PROJECT
----------------------------------------------------------------------------------------------------
[ ] 2025-01-12 14:30:15 Add user authentication to the app                 my-web-app
[ ] 2025-01-12 11:22:08 Fix the database connection timeout issue          backend-api
[ ] 2025-01-11 19:45:33 Refactor the payment module                        e-commerce
[ ] 2025-01-11 16:10:42 [Empty]                                            my-web-app
...

----------------------------------------------------------------------------------------------------
[j/k]Move [Space]Select [a]All [n]None [PgUp/PgDn]Page [q]Quit
```

## License

MIT
