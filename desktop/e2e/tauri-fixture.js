// Test-only transport injected by Playwright. Never imported by the application.
// These hosts are fixtures, not real infrastructure or credentials.
(() => {
  const hosts = [
    { id: "atlas", label: "Atlas Production", hostname: "atlas.example.test", port: 22, username: "deploy", auth: "agent", tags: ["fixture"] },
    { id: "orion", label: "Orion Staging", hostname: "orion.example.test", port: 22, username: "deploy", auth: "agent", tags: ["fixture"] },
  ];
  let sequence = 0;
  const callbacks = new Map();
  const listeners = new Map();
  const state = {
    calls: [], connects: [], closes: [], writes: [], live: {}, pending: [],
    holdNext: false, failNext: false,
    emit(event, payload) {
      for (const [id, entry] of listeners) if (entry.event === event)
        callbacks.get(entry.handler)?.({ event, id, payload });
    },
    output(id, text) { state.emit("session-data", { session_id: id, data: Array.from(new TextEncoder().encode(text)) }); },
    release() { state.pending.splice(0).forEach(resolve => resolve()); },
    disconnect(id) { delete state.live[id]; state.emit("session-closed", { session_id: id, reason: "Fixture disconnect" }); },
  };
  window.__terminalTest = state;
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, id) => listeners.delete(id) };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    transformCallback(callback) { const id = ++sequence; callbacks.set(id, callback); return id; },
    unregisterCallback(id) { callbacks.delete(id); },
    async invoke(command, args = {}) {
      state.calls.push({ command, args });
      if (command === "plugin:event|listen") {
        const id = ++sequence; listeners.set(id, args); return id;
      }
      if (command === "plugin:event|unlisten") { listeners.delete(args.eventId); return; }
      if (command === "list_hosts") return structuredClone(hosts);
      if (["list_groups", "list_identities", "list_keys", "list_workspaces", "list_known_hosts"].includes(command)) return [];
      if (["vault_is_initialized", "is_vault_unlocked"].includes(command)) return true;
      if (["biometric_available", "biometric_passphrase_stored"].includes(command)) return false;
      if (command === "get_app_info") return { name: "OpenTermius", version: "0.1.1-ui-test", platform: "linux", arch: "x86_64" };
      if (command === "check_for_updates") return { available: false, version: "0.1.1-ui-test", current_version: "0.1.1-ui-test", date: null, body: null };
      if (command === "connect_ssh" || command === "create_local_terminal") {
        const id = args.sessionId;
        const host = args.host;
        state.connects.push({ id, host: host?.id ?? "local" });
        if (state.holdNext) {
          state.holdNext = false;
          await new Promise(resolve => state.pending.push(resolve));
        }
        if (state.failNext) { state.failNext = false; throw new Error("Fixture: connection refused"); }
        state.live[id] = { host: host?.id ?? "local", input: "" };
        state.output(id, `\x1b[36mUI TEST FIXTURE — mocked ${host ? "SSH" : "local"} transport\x1b[0m\r\n${host?.username ?? "demo"}@${host?.hostname ?? "local"}\r\n$ `);
        return;
      }
      if (command === "close_session") {
        state.closes.push(args.sessionId);
        delete state.live[args.sessionId];
        return;
      }
      if (command === "session_resize") return;
      if (command === "session_write") {
        const text = new TextDecoder().decode(new Uint8Array(args.data));
        state.writes.push({ id: args.sessionId, text });
        const session = state.live[args.sessionId];
        if (!session) throw new Error("Write to a closed fixture session");
        for (const char of text) {
          if (char === "\r") {
            state.output(args.sessionId, `\r\n${session.input.startsWith("echo ") ? session.input.slice(5) : `/srv/${session.host}`}\r\n$ `);
            session.input = "";
          } else {
            session.input += char;
            state.output(args.sessionId, char);
          }
        }
        return;
      }
      if (["sftp_connect", "sftp_close", "secure_lock_vault"].includes(command)) return;
      if (command === "sftp_canonicalize") return "/srv/atlas";
      if (command === "sftp_list_dir") return [
        { name: "deployments", long_name: "deployments", is_dir: true, is_file: false, is_symlink: false, size: 4096, modified: 1788600000, permissions: 493 },
        { name: "logs", long_name: "logs", is_dir: true, is_file: false, is_symlink: false, size: 4096, modified: 1788600000, permissions: 493 },
        { name: "README.md", long_name: "README.md", is_dir: false, is_file: true, is_symlink: false, size: 2048, modified: 1788600000, permissions: 420 },
      ];
      throw new Error(`Unhandled test IPC: ${command}`);
    },
  };
})();
