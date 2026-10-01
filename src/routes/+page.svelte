<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { listen } from "@tauri-apps/api/event";
  import { LogicalSize } from "@tauri-apps/api/dpi";
  import { onMount } from "svelte";
  import Icon from "../lib/Icon.svelte";

  type Entry = {
    uuid: string;
    path: string;
    title: string;
    username: string;
    url: string;
    has_totp: boolean;
    icon: string | null;
    dup_index: number;
  };

  type Config = {
    db_path: string | null;
    clipboard_timeout_secs: number;
  };

  let win = getCurrentWindow();

  let mode = $state<"loading" | "setup" | "unlock" | "search">("loading");
  let config = $state<Config | null>(null);

  let password = $state("");
  let unlockError = $state("");
  let unlocking = $state(false);

  let query = $state("");
  let entries = $state<Entry[]>([]);
  let selected = $state(0);
  let toast = $state("");
  // Errors need more room than the header pill has, so they get their own row under the header.
  let notice = $state("");
  let copying = $state(false);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  let searchInput = $state<HTMLInputElement | null>(null);

  // Lower is better: title prefix < title word prefix < title contains < other fields.
  function termRank(e: Entry, term: string): number {
    const title = e.title.toLowerCase();
    if (title.startsWith(term)) return 0;
    if (title.split(/[\s\-_.\/]+/).some((w) => w.startsWith(term))) return 1;
    if (title.includes(term)) return 2;
    if ([e.username, e.url, e.path].some((f) => f.toLowerCase().includes(term))) return 3;
    return -1;
  }

  const matches = $derived.by(() => {
    const terms = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return [];
    const scored: { entry: Entry; score: number }[] = [];
    for (const entry of entries) {
      let score = 0;
      for (const term of terms) {
        const r = termRank(entry, term);
        if (r < 0) {
          score = -1;
          break;
        }
        score += r;
      }
      if (score >= 0) scored.push({ entry, score });
    }
    scored.sort((a, b) => a.score - b.score || a.entry.title.localeCompare(b.entry.title));
    return scored.map((s) => s.entry);
  });

  const MAX_RESULTS = 8;
  const results = $derived(matches.slice(0, MAX_RESULTS));
  const hiddenCount = $derived(matches.length - results.length);

  // Letter tile per entry (like 1Password's fallback icons): first letter on a
  // subtle tint whose hue is derived from the title, so the same entry always looks the same.
  function initialOf(title: string): string {
    return (title.trim()[0] ?? "?").toUpperCase();
  }
  function hueOf(title: string): number {
    let h = 0;
    for (const ch of title) h = (h * 31 + ch.charCodeAt(0)) % 360;
    return h;
  }

  // Group portion of the path ("Work/Dev/GitHub" -> "Work / Dev"); empty for top-level entries.
  function groupOf(e: Entry): string {
    const parts = e.path.split("/");
    return parts.slice(0, -1).join(" / ");
  }

  $effect(() => {
    if (selected >= results.length) selected = Math.max(0, results.length - 1);
  });

  async function resizeToContent() {
    const rowCount = mode === "search" ? results.length : 0;
    const headerHeight = 76;
    const rowHeight = 56;
    const footerHeight = mode === "search" && rowCount > 0 ? 34 : 0;
    const errorHeight = (mode === "unlock" && unlockError) || (mode === "search" && notice) ? 38 : 0;
    const height = headerHeight + rowCount * rowHeight + footerHeight + errorHeight;
    try {
      await win.setSize(new LogicalSize(680, Math.max(76, height)));
    } catch (err) {
      console.error("resizeToContent failed:", err);
    }
  }

  $effect(() => {
    results;
    mode;
    unlockError;
    notice;
    resizeToContent();
  });

  async function refresh() {
    config = await invoke<Config>("get_config");
    if (!config.db_path) {
      mode = "setup";
      return;
    }
    const unlocked = await invoke<boolean>("is_unlocked");
    if (unlocked) {
      entries = await invoke<Entry[]>("get_entries");
      mode = "search";
    } else {
      mode = "unlock";
    }
  }

  async function pickDatabase() {
    const path = await invoke<string | null>("pick_db_file");
    if (path) {
      await invoke("set_db_path", { path });
      config = await invoke<Config>("get_config");
      mode = "unlock";
      queueMicrotask(() => searchInput?.focus());
    }
  }

  const dbName = $derived(config?.db_path?.split("/").pop() ?? "");

  async function doUnlock(e: Event) {
    e.preventDefault();
    if (!password || unlocking) return;
    unlocking = true;
    unlockError = "";
    // (the field stays filled on failure so a typo can be fixed instead of retyped)
    try {
      entries = await invoke<Entry[]>("unlock", { password });
      password = "";
      mode = "search";
      queueMicrotask(() => searchInput?.focus());
    } catch (err) {
      unlockError = typeof err === "string" ? err : "Failed to unlock";
      queueMicrotask(() => searchInput?.select());
    } finally {
      unlocking = false;
    }
  }

  function showToast(message: string, isError = false) {
    clearTimeout(toastTimer);
    if (isError) {
      toast = "";
      notice = message;
      toastTimer = setTimeout(() => (notice = ""), 4000);
    } else {
      notice = "";
      toast = message;
      toastTimer = setTimeout(() => (toast = ""), 1600);
    }
  }

  async function copy(kind: "password" | "username" | "totp", entry: Entry) {
    if (copying) return;
    if (entry.dup_index > 0) {
      showToast("Same name twice in this group. Rename one in KeePassXC.", true);
      return;
    }
    const cmd = kind === "password" ? "copy_password" : kind === "username" ? "copy_username" : "copy_totp";
    copying = true;
    notice = "";
    try {
      await invoke(cmd, { entryPath: entry.path });
      showToast(`${kind === "totp" ? "TOTP" : kind[0].toUpperCase() + kind.slice(1)} copied`);
      setTimeout(() => invoke("hide_main_window"), 250);
    } catch (err) {
      showToast(typeof err === "string" ? err : "Copy failed", true);
    } finally {
      copying = false;
    }
  }

  function onSearchKeydown(e: KeyboardEvent) {
    if (copying && e.key !== "Escape") {
      e.preventDefault();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selected = Math.min(selected + 1, results.length - 1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selected = Math.max(selected - 1, 0);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const entry = results[selected];
      if (entry) copy("password", entry);
    } else if (e.key === "Escape") {
      e.preventDefault();
      invoke("hide_main_window");
    } else if (e.key.toLowerCase() === "u" && e.metaKey) {
      e.preventDefault();
      const entry = results[selected];
      if (entry) copy("username", entry);
    } else if (e.key.toLowerCase() === "t" && e.metaKey) {
      e.preventDefault();
      const entry = results[selected];
      if (entry?.has_totp) copy("totp", entry);
    }
  }

  onMount(() => {
    refresh();

    const unlistenShown = listen("window-shown", () => {
      query = "";
      selected = 0;
      queueMicrotask(() => searchInput?.focus());
    });

    // Also fires after the database was switched from the tray, so re-read the config.
    const unlistenLocked = listen("locked", () => {
      entries = [];
      password = "";
      unlockError = "";
      refresh().then(() => queueMicrotask(() => searchInput?.focus()));
    });

    return () => {
      unlistenShown.then((f) => f());
      unlistenLocked.then((f) => f());
    };
  });
</script>

<div class="shell">
  {#if mode === "setup"}
    <div class="row header">
      <span class="icon"><Icon name="lock" /></span>
      <div class="setup-text">
        <div class="title">No database selected</div>
        <div class="subtitle">Choose your KeePassXC .kdbx file to get started</div>
      </div>
      <button class="pick-btn" onclick={pickDatabase}>Choose…</button>
    </div>
  {:else if mode === "unlock"}
    <form class="row header" class:shake={unlockError} onsubmit={doUnlock}>
      <span class="icon"><Icon name="lock" /></span>
      <input bind:this={searchInput} type="password" class="query" placeholder="Master password" bind:value={password} />
      {#if unlocking}
        <span class="spinner"></span>
      {:else}
        <span class="db-name" title={config?.db_path ?? ""}>{dbName}</span>
        <button type="button" class="pick-btn" onclick={pickDatabase}>Change…</button>
      {/if}
    </form>
    {#if unlockError}
      <div class="error-row">{unlockError}</div>
    {/if}
  {:else if mode === "search"}
    <div class="row header">
      <span class="icon"><Icon name="search" /></span>
      <input bind:this={searchInput} class="query" placeholder="Search KeeSpot…" bind:value={query} onkeydown={onSearchKeydown} />
      {#if copying}
        <span class="spinner"></span>
      {:else if toast}
        <span class="toast">{toast}</span>
      {/if}
    </div>

    {#if notice}
      <div class="error-row notice">{notice}</div>
    {/if}

    {#if results.length > 0}
      <ul class="results">
        {#each results as entry, i (entry.uuid || `${entry.path}#${entry.dup_index}`)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <li
            class="result"
            class:active={i === selected}
            role="option"
            aria-selected={i === selected}
            onmouseenter={() => (selected = i)}
            onclick={() => copy("password", entry)}
          >
            {#if entry.icon}
              <img class="result-icon favicon" src={entry.icon} alt="" />
            {:else}
              <span class="result-icon" style="--hue: {hueOf(entry.title)}">{initialOf(entry.title)}</span>
            {/if}
            <div class="result-text">
              <div class="result-title">{entry.title}</div>
              <div class="result-subtitle">{entry.username || entry.path}</div>
            </div>
            {#if groupOf(entry)}
              <span class="group">{groupOf(entry)}</span>
            {/if}
            {#if entry.has_totp}
              <span class="badge">TOTP</span>
            {/if}
          </li>
        {/each}
      </ul>
      <div class="footer">
        <span>↵ copy password</span>
        <span>⌘U username</span>
        {#if results[selected]?.has_totp}<span>⌘T totp</span>{/if}
        {#if hiddenCount > 0}<span class="more">+{hiddenCount} more</span>{/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  /* Theme tokens: follow the OS appearance automatically (no settings page needed). */
  :global(:root) {
    --bg: rgba(246, 246, 248, 0.74);
    --fg: #1d1d1f;
    --fg-muted: color-mix(in srgb, var(--fg) 55%, transparent);
    --hover: color-mix(in srgb, var(--fg) 10%, transparent);
    --border: rgba(127, 127, 127, 0.22);
    --divider: rgba(127, 127, 127, 0.18);
    --accent: #0a84ff;
    --accent-fg: #fff;
    --danger: #ff453a;
    --success: #34c759;
    --shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
  }

  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --bg: rgba(28, 28, 30, 0.78);
      --fg: #f5f5f7;
      --border: rgba(255, 255, 255, 0.14);
      --divider: rgba(255, 255, 255, 0.1);
      --shadow: 0 20px 60px rgba(0, 0, 0, 0.6);
    }
  }

  :global(html),
  :global(body) {
    margin: 0;
    background: transparent;
    overflow: hidden;
  }

  :global(*) {
    box-sizing: border-box;
  }

  .shell {
    /* Scoped here, not on :root: a root color-scheme makes WebKit paint an opaque page canvas behind the rounded corners. */
    color-scheme: light dark;
    width: 680px;
    font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif;
    background: var(--bg);
    backdrop-filter: blur(30px) saturate(180%);
    -webkit-backdrop-filter: blur(30px) saturate(180%);
    border-radius: 14px;
    /* WebKit can paint the backdrop blur as a full rectangle, ignoring border-radius; clip-path forces the rounded shape. */
    clip-path: inset(0 round 14px);
    border: 1px solid var(--border);
    box-shadow: var(--shadow);
    color: var(--fg);
    overflow: hidden;
  }

  .header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 20px;
    height: 76px;
  }

  .icon {
    display: flex;
    flex-shrink: 0;
    opacity: 0.85;
  }

  .query {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 24px;
    font-weight: 400;
    color: inherit;
  }

  .query::placeholder {
    color: var(--fg-muted);
  }

  .setup-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .title {
    font-size: 15px;
    font-weight: 600;
  }

  .subtitle {
    font-size: 12px;
    opacity: 0.6;
  }

  .pick-btn {
    border: none;
    border-radius: 8px;
    background: var(--hover);
    color: inherit;
    padding: 8px 14px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }

  .pick-btn:hover {
    background: color-mix(in srgb, var(--fg) 16%, transparent);
  }

  .db-name {
    font-size: 12px;
    color: var(--fg-muted);
    max-width: 160px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 0;
  }

  .error-row.notice {
    padding-top: 10px;
    border-top: 1px solid var(--divider);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .error-row {
    padding: 0 20px 14px;
    font-size: 12px;
    color: var(--danger);
  }

  .spinner {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, var(--fg) 20%, transparent);
    border-top-color: color-mix(in srgb, var(--fg) 70%, transparent);
    animation: spin 0.7s linear infinite;
    flex-shrink: 0;
  }

  .shake {
    animation: shake 0.3s;
  }

  @keyframes shake {
    25% {
      transform: translateX(-6px);
    }
    75% {
      transform: translateX(6px);
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .toast {
    font-size: 12px;
    font-weight: 500;
    padding: 4px 10px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--success) 25%, transparent);
    color: var(--fg);
    flex-shrink: 0;
  }

  .results {
    list-style: none;
    margin: 0;
    padding: 6px;
    border-top: 1px solid var(--divider);
    max-height: 448px;
    overflow-y: auto;
  }

  .result {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 50px;
    padding: 0 10px;
    border-radius: 8px;
    cursor: pointer;
  }

  .result.active {
    background: color-mix(in srgb, var(--accent) 85%, transparent);
    color: var(--accent-fg);
  }

  .result-icon {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    font-weight: 600;
    flex-shrink: 0;
    background: hsl(var(--hue) 55% 50% / 0.22);
    color: hsl(var(--hue) 60% 45%);
  }

  @media (prefers-color-scheme: dark) {
    .result-icon {
      color: hsl(var(--hue) 75% 72%);
    }
  }

  .favicon {
    object-fit: contain;
    padding: 4px;
    background: rgba(255, 255, 255, 0.9);
  }

  .result.active .favicon {
    background: rgba(255, 255, 255, 0.9);
  }

  .result.active .result-icon:not(.favicon) {
    background: rgba(255, 255, 255, 0.25);
    color: #fff;
  }

  .result-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }

  .result-title {
    font-size: 14px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-subtitle {
    font-size: 12px;
    opacity: 0.6;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result.active .result-subtitle {
    opacity: 0.85;
  }

  .group {
    font-size: 12px;
    opacity: 0.5;
    white-space: nowrap;
    flex-shrink: 0;
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result.active .group {
    opacity: 0.8;
  }

  .badge {
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 2px 6px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--fg) 14%, transparent);
    flex-shrink: 0;
  }

  .result.active .badge {
    background: rgba(255, 255, 255, 0.25);
  }

  .more {
    margin-left: auto;
  }

  .footer {
    display: flex;
    gap: 16px;
    padding: 8px 16px;
    border-top: 1px solid var(--divider);
    font-size: 11px;
    opacity: 0.55;
  }
</style>
