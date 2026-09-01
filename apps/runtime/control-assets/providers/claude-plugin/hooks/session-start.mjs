const additionalContext = `Matou host control is active because the matou-host-control plugin is loaded.

For any request about another, adjacent, left, or right Matou card, session, or terminal, invoke \`matou-host-control:mt-terminal\` immediately and follow its \`mt identify -> mt list -> mt read/history/commands\` flow.

Treat Matou's \`mt\` control plane as authoritative for these requests. Skip alternate terminal host environment discovery and other terminal-host probes in this Matou-managed session.`

process.stdout.write(JSON.stringify({
  hookSpecificOutput: {
    hookEventName: 'SessionStart',
    additionalContext
  }
}))
