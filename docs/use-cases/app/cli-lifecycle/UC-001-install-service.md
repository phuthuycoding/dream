## UC-001 Install and start the daemon service

Owner runs `dream up`. Daemon binary path, port, bind, data-dir and log flags are written to `~/Library/LaunchAgents/com.dream.daemon.plist`; the agent bootstraps, token URL + logs path + tailscale hint print. Re-running replaces the agent (idempotent).
