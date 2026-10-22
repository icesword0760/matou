# Independent session transcript persistence

## User-visible defect and fix

Launching Matou from inside an agent's shell could pass that agent's child-session
markers to every terminal card. The official Claude CLI treats this as a nested
session and suppresses transcript persistence. Terminal output still appears, but
after restarting the app the CLI resumes the last previously saved turn.

Each independent PTY now strips only the caller's identity/nesting markers after
all environment layers have merged. Shell, Claude, Codex and resumed cards share
this boundary. Authentication, configuration directories, proxies and explicit
privacy settings are retained. No transcript rewrite or account migration occurs.

## Verification (2026-10-15)

| Scenario | Result / evidence |
| --- | --- |
| Polluted parent environment, all three terminal profiles | Real PTY tests pass; caller markers absent, fixture authentication and card controls retained |
| Launch desktop with caller markers | Unmodified official Claude CLI 2.1.299 saves first user/assistant turn |
| Restart with caller markers | Same provider ID restores first turn; second request contains prior context |
| Normal desktop restart | Third request contains both prior turns; all three turns remain in official JSONL |
| Signed packaged macOS app | Same three-turn/two-restart test passes (10.5 seconds) |
| Login isolation | Test uses disposable HOME/config, local model server and synthetic key; sandbox denies real CLI config and Keychain command access |
| Regression | Runtime targeted suite 65 tests; account suite 31 TypeScript + 27 Python tests; HUD suite 46 tests; workspace typecheck and identifier gate pass |

This is an official CLI persistence regression, not a reference-product UI parity
change. No menus, card placement, account interaction or visual behavior change.

## Reproduce

Build first, then run the opt-in Playwright test with
`MATOU_TEST_CLAUDE_COMMAND` set to an absolute official CLI executable path:

```sh
pnpm exec playwright test tests/e2e/real-claude-independent-launch.spec.ts --workers=1
```

Set `MATOU_TEST_PACKAGED_APP` to the packaged application's executable to test the
release candidate instead of the development build. Test output includes three
screenshots and `persistence-proof.json` with provider identity and spawn markers.

Lost content can be summarized from the local terminal journal when available.
Such a summary is explicitly labeled reconstructed context and sent as a new
message to the same conversation, not inserted as fabricated original history.
This does not recreate original image attachments or guarantee a verbatim record.

## Installed-app readback

With user approval, the local candidate replaced the installed app after a normal
quit. The previous app and a coherent database/WAL snapshot were preserved; the
backup database passed `quick_check`. Installed `app.asar` matches the tested
candidate and its code signature verifies.

The affected conversation acknowledged the reconstructed context before shutdown.
After replacement, its latest acknowledgement is visible in the same card. All
113 session identities/ownership records, 100 provider bindings, 96 canvas
memberships, 16 workspace identities and 28 task identities match the pre-restart
snapshot. The eight account-store files also match. The account panel shows the
same default account and cached usage. No account switch or login was performed.
Independent restarted PTYs have no inherited caller-session markers.

This local acceptance build remains version 0.2.15; no GitHub release or remote
deployment was changed as part of this fix.
