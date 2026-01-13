use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use clap::Parser;
use colored::Colorize;
use console::{Key, Term};
use dialoguer::{theme::ColorfulTheme, Confirm};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(author, version, about = "Manage and clean Claude Code conversation history")]
struct Args {
    #[arg(short, long, help = "Filter by workspace (e.g., myproject)")]
    workspace: Option<String>,

    #[arg(short, long, help = "Only show empty conversations")]
    empty_only: bool,

    #[arg(long, help = "Delete all empty (0-byte) conversations")]
    delete_empty: bool,

    #[arg(long, help = "Also delete warmup agent files (use with caution)")]
    delete_warmup: bool,

    #[arg(short, long, help = "List all workspaces")]
    list_workspaces: bool,

    #[arg(long, help = "Include warmup/subagent conversations")]
    include_agents: bool,
}

#[derive(Debug, Clone)]
struct Conversation {
    path: PathBuf,
    session_id: String,
    workspace_folder: PathBuf,
    workspace_path: String,
    is_empty: bool,
    is_active: bool,
    title: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    folder_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct JsonlEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    message: Option<Message>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<serde_json::Value>,
}

fn get_claude_projects_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let projects_dir = home.join(".claude").join("projects");
    if !projects_dir.exists() {
        anyhow::bail!("Claude projects directory not found at: {}", projects_dir.display());
    }
    Ok(projects_dir)
}

fn decode_workspace_name(name: &str) -> String {
    if name.starts_with('-') {
        name.replacen('-', "/", 1).replace('-', "/")
    } else {
        name.replace('-', "/")
    }
}

fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<JsonlEntry>(line) {
            if entry.entry_type.as_deref() == Some("user") {
                if let Some(msg) = entry.message {
                    if let Some(content) = msg.content {
                        let text = extract_text_from_content(&content);
                        if !text.is_empty() && text != "Warmup" && !text.starts_with("<ide_") {
                            let title: String = text.chars().take(50).collect();
                            return Some(if text.chars().count() > 50 {
                                format!("{}...", title)
                            } else {
                                title
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_text_from_content(content: &serde_json::Value) -> String {
    let raw = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut result = String::new();
            for item in arr {
                if let Some(obj) = item.as_object() {
                    if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                            if !text.starts_with("<ide_") {
                                result = text.to_string();
                                break;
                            }
                        }
                    }
                }
            }
            result
        }
        _ => String::new(),
    };
    // Only take first line and clean up whitespace
    raw.lines()
        .next()
        .unwrap_or("")
        .trim()
        .replace('\t', " ")
        .to_string()
}

fn extract_timestamp(content: &str) -> Option<DateTime<Utc>> {
    let mut last_timestamp: Option<DateTime<Utc>> = None;
    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<JsonlEntry>(line) {
            if let Some(ts) = entry.timestamp {
                if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
                    last_timestamp = Some(dt);
                }
            }
        }
    }
    last_timestamp
}

fn is_warmup_only(content: &str) -> bool {
    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<JsonlEntry>(line) {
            if entry.entry_type.as_deref() == Some("user") {
                if let Some(msg) = entry.message {
                    if let Some(content) = msg.content {
                        let text = extract_text_from_content(&content);
                        if !text.is_empty() && text != "Warmup" && !text.starts_with("<ide_") {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn scan_conversations(projects_dir: &Path, workspace_filter: Option<&str>, include_agents: bool) -> Result<Vec<Conversation>> {
    let mut conversations = Vec::new();

    for entry in fs::read_dir(projects_dir)? {
        let entry = entry?;
        let workspace_folder = entry.path();
        if !workspace_folder.is_dir() { continue; }

        let workspace_name = workspace_folder.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let workspace_path = decode_workspace_name(&workspace_name);

        if let Some(filter) = workspace_filter {
            if !workspace_path.contains(filter) && !workspace_name.contains(filter) {
                continue;
            }
        }

        for file_entry in fs::read_dir(&workspace_folder)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();

            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let file_name = file_path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
            let is_agent = file_name.starts_with("agent-");

            // Skip agent files unless explicitly included
            if is_agent && !include_agents {
                continue;
            }

            let metadata = fs::metadata(&file_path)?;
            let size = metadata.len();
            let is_empty = size == 0;

            // Check if file was modified in last 5 minutes (likely active)
            let is_active = metadata.modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() < 300)
                .unwrap_or(false);

            let session_id = file_name.to_string();
            let folder_path = workspace_folder.join(&session_id);
            let folder_exists = folder_path.is_dir();

            let (title, timestamp, is_warmup) = if !is_empty {
                let content = fs::read_to_string(&file_path).unwrap_or_default();
                let t = extract_title(&content);
                let ts = extract_timestamp(&content);
                let warmup = is_warmup_only(&content);
                (t, ts, warmup)
            } else {
                (None, None, false)
            };

            // For agent files, mark as warmup if they only contain warmup messages
            let effective_title = if is_agent && is_warmup {
                Some("[Warmup]".to_string())
            } else {
                title
            };

            conversations.push(Conversation {
                path: file_path,
                session_id,
                workspace_folder: workspace_folder.clone(),
                workspace_path: workspace_path.clone(),
                is_empty,
                is_active,
                title: effective_title,
                timestamp,
                folder_path: if folder_exists { Some(folder_path) } else { None },
            });
        }
    }

    // Sort: has title first, then no title, then empty. Within each group: by timestamp desc
    conversations.sort_by(|a, b| {
        // Priority: has_title > no_title > empty
        let priority = |c: &Conversation| {
            if c.is_empty { 2 }
            else if c.title.is_none() || c.title.as_deref() == Some("[No title]") { 1 }
            else { 0 }
        };
        let pa = priority(a);
        let pb = priority(b);
        if pa != pb {
            return pa.cmp(&pb);
        }
        // Within same priority: by timestamp (newest first, None at end)
        match (&b.timestamp, &a.timestamp) {
            (Some(tb), Some(ta)) => tb.cmp(ta),
            (Some(_), None) => std::cmp::Ordering::Less,    // b has time, a doesn't -> b first
            (None, Some(_)) => std::cmp::Ordering::Greater, // a has time, b doesn't -> a first
            (None, None) => a.path.cmp(&b.path),
        }
    });

    Ok(conversations)
}

fn list_workspaces(projects_dir: &Path) -> Result<()> {
    println!("{}", "Available workspaces:".bold().cyan());
    println!();

    let mut workspaces: Vec<(String, String, usize, usize)> = Vec::new();

    for entry in fs::read_dir(projects_dir)? {
        let entry = entry?;
        let workspace_folder = entry.path();
        if !workspace_folder.is_dir() { continue; }

        let workspace_name = workspace_folder.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let workspace_path = decode_workspace_name(&workspace_name);

        let mut total = 0;
        let mut agents = 0;
        for e in fs::read_dir(&workspace_folder)?.filter_map(|e| e.ok()) {
            if e.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                total += 1;
                if e.path().file_stem().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("agent-")) {
                    agents += 1;
                }
            }
        }

        workspaces.push((workspace_name, workspace_path, total, agents));
    }

    workspaces.sort_by(|a, b| a.1.cmp(&b.1));

    for (name, path, total, agents) in workspaces {
        let main_count = total - agents;
        println!("  {} {} ({} chats, {} agents)", "->".green(), path, main_count.to_string().yellow(), agents.to_string().dimmed());
        println!("     {}", format!("-w {}", name).dimmed());
    }

    Ok(())
}

/// Delete conversation and its related agent files
fn delete_conversation_with_agents(conv: &Conversation) -> Result<usize> {
    let mut deleted = 1;

    // Delete main file
    fs::remove_file(&conv.path).with_context(|| format!("Failed to delete {}", conv.path.display()))?;

    // Delete associated folder
    if let Some(ref folder) = conv.folder_path {
        if folder.exists() {
            fs::remove_dir_all(folder).with_context(|| format!("Failed to delete folder {}", folder.display()))?;
        }
    }

    // Delete related agent files (agent files that reference this session_id)
    // Agent files have sessionId field that matches the main conversation's file name
    if !conv.session_id.starts_with("agent-") {
        // This is a main conversation, find and delete related agents
        for entry in fs::read_dir(&conv.workspace_folder)?.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("agent-") {
                continue;
            }

            // Check if this agent belongs to our conversation
            if let Ok(content) = fs::read_to_string(&path) {
                if let Some(first_line) = content.lines().next() {
                    if let Ok(entry) = serde_json::from_str::<JsonlEntry>(first_line) {
                        if entry.session_id.as_deref() == Some(&conv.session_id) {
                            // This agent belongs to our conversation
                            if fs::remove_file(&path).is_ok() {
                                deleted += 1;
                                // Also delete agent folder if exists
                                let agent_folder = conv.workspace_folder.join(name);
                                if agent_folder.is_dir() {
                                    let _ = fs::remove_dir_all(&agent_folder);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(deleted)
}

fn get_display_title(conv: &Conversation) -> String {
    if conv.is_empty {
        "[Empty]".to_string()
    } else if let Some(ref t) = conv.title {
        t.clone()
    } else {
        "[No title]".to_string()
    }
}

fn get_short_workspace(path: &str) -> String {
    path.split('/').last().unwrap_or(path).to_string()
}

fn run_interactive(conversations: Vec<Conversation>) -> Result<()> {
    if conversations.is_empty() {
        println!("{}", "No conversations found.".yellow());
        return Ok(());
    }

    let empty_count = conversations.iter().filter(|c| c.is_empty).count();
    let warmup_count = conversations.iter().filter(|c| c.title.as_deref() == Some("[Warmup]")).count();
    let total = conversations.len();

    println!();
    println!("Found {} conversations", total.to_string().bold());
    if empty_count > 0 {
        println!("  {} empty (0-byte files, safe to delete)", empty_count.to_string().red());
    }
    if warmup_count > 0 {
        println!("  {} warmup agents (cache warming, usually safe)", warmup_count.to_string().yellow());
    }
    println!();

    let mut remaining = conversations;

    // Ask about empty files first (always safe)
    if empty_count > 0 {
        println!("{}", "Empty conversations (0-byte, safe to delete):".yellow());
        let to_delete: Vec<&Conversation> = remaining.iter().filter(|c| c.is_empty).collect();
        for conv in &to_delete {
            println!("  - {} ({})", conv.session_id.dimmed(), get_short_workspace(&conv.workspace_path));
        }
        println!();

        let cleanup = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Delete {} empty conversations?", empty_count))
            .default(true)
            .interact()?;

        if cleanup {
            let mut deleted = 0;
            let mut errors = 0;
            for conv in &to_delete {
                match delete_conversation_with_agents(conv) {
                    Ok(_) => deleted += 1,
                    Err(e) => {
                        eprintln!("  {} Failed to delete {}: {}", "ERR".red(), conv.session_id, e);
                        errors += 1;
                    }
                }
            }
            if errors > 0 {
                println!("{} Deleted {} empty conversations ({} failed)", "WARN".yellow(), deleted, errors);
            } else {
                println!("{} Deleted {} empty conversations", "OK".green(), deleted);
            }
            remaining.retain(|c| !c.is_empty);
        }
    }

    // Warmup is separate - user needs to consciously choose
    let warmup_in_remaining = remaining.iter().filter(|c| c.title.as_deref() == Some("[Warmup]")).count();
    if warmup_in_remaining > 0 {
        println!();
        println!("{}", "Warmup agents (cache files, usually safe):".yellow());
        let to_delete: Vec<&Conversation> = remaining.iter()
            .filter(|c| c.title.as_deref() == Some("[Warmup]"))
            .collect();
        for conv in &to_delete {
            println!("  - {} ({})", conv.session_id.dimmed(), get_short_workspace(&conv.workspace_path));
        }
        println!();

        let cleanup_warmup = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Delete {} warmup agents?", warmup_in_remaining))
            .default(false)
            .interact()?;

        if cleanup_warmup {
            let mut deleted = 0;
            let mut errors = 0;
            for conv in &to_delete {
                match delete_conversation_with_agents(conv) {
                    Ok(_) => deleted += 1,
                    Err(e) => {
                        eprintln!("  {} Failed to delete {}: {}", "ERR".red(), conv.session_id, e);
                        errors += 1;
                    }
                }
            }
            if errors > 0 {
                println!("{} Deleted {} warmup agents ({} failed)", "WARN".yellow(), deleted, errors);
            } else {
                println!("{} Deleted {} warmup agents", "OK".green(), deleted);
            }
            remaining.retain(|c| c.title.as_deref() != Some("[Warmup]"));
        }
    }

    if remaining.is_empty() {
        println!();
        println!("{}", "No more conversations.".yellow());
        return Ok(());
    }

    run_selection(remaining)
}

fn run_selection(conversations: Vec<Conversation>) -> Result<()> {
    if conversations.is_empty() { return Ok(()); }

    let term = Term::stdout();
    let mut cursor: usize = 0;
    let mut selected: Vec<bool> = vec![false; conversations.len()];
    let mut viewport_start: usize = 0;

    // Count active conversations
    let active_count = conversations.iter().filter(|c| c.is_active).count();

    // Clear screen and hide cursor
    let _ = term.clear_screen();
    let _ = term.hide_cursor();

    loop {
        // Get terminal height and calculate viewport
        let term_height = term.size().0 as usize;
        let header_lines = if active_count > 0 { 8 } else { 7 }; // +1 for active line
        let footer_lines = 3;
        let viewport_size = term_height.saturating_sub(header_lines + footer_lines).max(3);

        // Adjust viewport to follow cursor (smooth scrolling)
        if cursor < viewport_start {
            viewport_start = cursor;
        } else if cursor >= viewport_start + viewport_size {
            viewport_start = cursor - viewport_size + 1;
        }

        // Move to top and clear
        let _ = term.move_cursor_to(0, 0);
        let _ = term.clear_screen();

        let selected_count = selected.iter().filter(|&&s| s).count();
        let viewport_end = std::cmp::min(viewport_start + viewport_size, conversations.len());

        println!("{}", "Claude Code Chat Manager".bold().cyan());
        println!("Total: {} | Selected: {} | Showing: {}-{}/{}",
            conversations.len(),
            selected_count.to_string().yellow(),
            (viewport_start + 1).to_string().cyan(),
            viewport_end.to_string().cyan(),
            conversations.len()
        );
        if active_count > 0 {
            println!("{}", format!("  {} active (modified <5min, marked with *)", active_count).yellow());
        }
        println!();

        println!(
            "{:3} {:19} {:50} {}",
            "".dimmed(),
            "LAST ACTIVE".dimmed(),
            "TITLE".dimmed(),
            "PROJECT".dimmed()
        );
        println!("{}", "-".repeat(100).dimmed());

        for i in viewport_start..viewport_end {
            let conv = &conversations[i];
            let is_cur = i == cursor;
            let is_sel = selected[i];

            let checkbox = if is_sel {
                "[/]".green().bold().to_string()
            } else {
                "[ ]".to_string()
            };

            let time_str = conv.timestamp
                .map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "---".to_string());

            let title = get_display_title(conv);
            let active_marker = if conv.is_active { "*" } else { "" };
            let title_with_marker = format!("{}{}", active_marker, title);
            let title_display: String = title_with_marker.chars().take(48).collect();

            let project = get_short_workspace(&conv.workspace_path);

            if is_cur {
                if conv.is_active {
                    println!(
                        "{} {} {} {}",
                        checkbox.on_bright_black(),
                        time_str.red().bold(),
                        format!("{:<48}", title_display).red().bold(),
                        project.cyan().bold()
                    );
                } else {
                    println!(
                        "{} {} {} {}",
                        checkbox.on_bright_black(),
                        time_str.yellow().bold(),
                        format!("{:<48}", title_display).white().bold(),
                        project.cyan().bold()
                    );
                }
            } else if is_sel {
                println!(
                    "{} {} {} {}",
                    checkbox,
                    time_str.yellow(),
                    format!("{:<48}", title_display).white(),
                    project.cyan()
                );
            } else if conv.is_active {
                println!(
                    "{} {} {} {}",
                    checkbox.dimmed(),
                    time_str.red(),
                    format!("{:<48}", title_display).red(),
                    project.dimmed()
                );
            } else {
                println!(
                    "{} {} {} {}",
                    checkbox.dimmed(),
                    time_str,
                    format!("{:<48}", title_display),
                    project.dimmed()
                );
            }
        }

        println!();
        println!("{}", "-".repeat(100).dimmed());

        if selected_count > 0 {
            println!(
                "{} {}",
                format!("Delete {} chat(s)?", selected_count).red().bold(),
                "[ENTER=Delete] [ESC=Cancel]".dimmed()
            );
        } else {
            println!(
                "{} {} {} {} {} {}",
                "[j/k]Move".dimmed(),
                "[Space]Select".dimmed(),
                "[a]All".dimmed(),
                "[n]None".dimmed(),
                "[PgUp/PgDn]Page".dimmed(),
                "[q]Quit".dimmed()
            );
        }

        match term.read_key()? {
            Key::ArrowUp | Key::Char('k') => {
                if cursor > 0 { cursor -= 1; }
            }
            Key::ArrowDown | Key::Char('j') => {
                if cursor < conversations.len() - 1 { cursor += 1; }
            }
            Key::Char(' ') => {
                selected[cursor] = !selected[cursor];
                if cursor < conversations.len() - 1 { cursor += 1; }
            }
            Key::Char('a') => {
                for s in selected.iter_mut() { *s = true; }
            }
            Key::Char('n') => {
                for s in selected.iter_mut() { *s = false; }
            }
            Key::PageUp => {
                cursor = cursor.saturating_sub(viewport_size);
            }
            Key::PageDown => {
                cursor = std::cmp::min(cursor + viewport_size, conversations.len() - 1);
            }
            Key::Enter => {
                let indices: Vec<usize> = selected.iter().enumerate()
                    .filter(|&(_, s)| *s).map(|(i, _)| i).collect();

                if !indices.is_empty() {
                    // Check for active conversations
                    let active_selected: Vec<usize> = indices.iter()
                        .filter(|&&i| conversations[i].is_active)
                        .copied()
                        .collect();

                    // Direct Enter to delete - final confirmation screen
                    let _ = term.clear_screen();

                    println!("{}", "Claude Code Chat Manager".bold().cyan());
                    println!();

                    if !active_selected.is_empty() {
                        println!("{}", format!("WARNING: {} conversation(s) may be currently in use!", active_selected.len()).red().bold());
                        println!("{}", "(Modified within last 5 minutes)".red());
                        println!();
                    }

                    println!("{} conversations to delete:", indices.len().to_string().red().bold());
                    println!();

                    for &i in &indices {
                        let c = &conversations[i];
                        let active_mark = if c.is_active { " [ACTIVE]".red().to_string() } else { "".to_string() };
                        println!("  - {}{} ({})", get_display_title(c), active_mark, c.workspace_path.dimmed());
                    }

                    println!();
                    if !active_selected.is_empty() {
                        println!("{}", "Press ENTER to confirm (may cause errors in Claude Code), ESC to cancel".yellow());
                    } else {
                        println!("{}", "Press ENTER to confirm, ESC to cancel".yellow());
                    }

                    // Wait for final confirmation
                    loop {
                        match term.read_key()? {
                            Key::Enter => {
                                let mut total_deleted = 0;
                                let mut errors = 0;
                                println!();
                                for &i in &indices {
                                    let conv = &conversations[i];
                                    match delete_conversation_with_agents(conv) {
                                        Ok(n) => {
                                            total_deleted += n;
                                            println!("  {} {}", "OK".green(), get_display_title(conv).dimmed());
                                        }
                                        Err(e) => {
                                            eprintln!("  {} {} - {}", "ERR".red(), conv.session_id, e);
                                            errors += 1;
                                        }
                                    }
                                }
                                println!();
                                if errors > 0 {
                                    println!("{} Deleted {} files ({} failed)",
                                        "WARN".yellow().bold(),
                                        total_deleted.to_string().green(),
                                        errors.to_string().red()
                                    );
                                } else {
                                    println!("{} Deleted {} files ({} chats + related agents)",
                                        "OK".green().bold(),
                                        total_deleted.to_string().green(),
                                        indices.len()
                                    );
                                }
                                println!();
                                println!("Press any key to exit...");
                                let _ = term.read_key();
                                let _ = term.clear_screen();
                                let _ = term.show_cursor();
                                return Ok(());
                            }
                            Key::Escape => {
                                // Cancel and go back
                                for s in selected.iter_mut() { *s = false; }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            Key::Escape | Key::Char('q') => {
                let _ = term.clear_screen();
                let _ = term.show_cursor();
                println!("Cancelled.");
                return Ok(());
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let projects_dir = get_claude_projects_dir()?;

    if args.list_workspaces {
        println!();
        println!("{}", "Claude Code Chat Manager".bold().cyan());
        println!();
        return list_workspaces(&projects_dir);
    }

    if args.delete_empty || args.delete_warmup {
        println!();
        println!("{}", "Claude Code Chat Manager".bold().cyan());
        println!();

        let conversations = scan_conversations(&projects_dir, args.workspace.as_deref(), true)?;

        let to_delete: Vec<&Conversation> = conversations.iter()
            .filter(|c| {
                (args.delete_empty && c.is_empty) ||
                (args.delete_warmup && c.title.as_deref() == Some("[Warmup]"))
            })
            .collect();

        if to_delete.is_empty() {
            println!("{}", "No matching conversations found.".yellow());
            return Ok(());
        }

        let empty_count = to_delete.iter().filter(|c| c.is_empty).count();
        let warmup_count = to_delete.iter().filter(|c| c.title.as_deref() == Some("[Warmup]")).count();

        // Show the list first
        println!("Found {} conversations to delete ({} empty, {} warmup):",
            to_delete.len().to_string().red(),
            empty_count,
            warmup_count
        );
        println!();
        for conv in &to_delete {
            let label = if conv.is_empty { "[Empty]" } else { "[Warmup]" };
            println!("  - {} {} ({})",
                label.dimmed(),
                conv.session_id.dimmed(),
                get_short_workspace(&conv.workspace_path)
            );
        }
        println!();

        // Ask for confirmation
        let confirm = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("Delete {} conversations?", to_delete.len()))
            .default(false)
            .interact()?;

        if !confirm {
            println!("{}", "Cancelled.".yellow());
            return Ok(());
        }

        let mut deleted = 0;
        let mut errors = 0;
        for conv in to_delete {
            match delete_conversation_with_agents(conv) {
                Ok(_) => {
                    deleted += 1;
                    println!("  {} {}", "OK".green(), conv.session_id.dimmed());
                }
                Err(e) => {
                    eprintln!("  {} {} - {}", "ERR".red(), conv.session_id, e);
                    errors += 1;
                }
            }
        }

        if errors > 0 {
            println!("{} Done! Deleted {} ({} failed)", "WARN".yellow(), deleted, errors);
        } else {
            println!("{}", format!("Done! Deleted {} conversations.", deleted).green().bold());
        }
        return Ok(());
    }

    let mut conversations = scan_conversations(&projects_dir, args.workspace.as_deref(), args.include_agents)?;

    if args.empty_only {
        conversations.retain(|c| c.is_empty);
    }

    run_interactive(conversations)
}
