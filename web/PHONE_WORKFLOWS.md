# Phone web: first workflow slice (not a mobile desktop replacement)

The `/workflows` route is a responsive early-access slice backed by the existing community relay, not a demo backend. Open it on a relay host serving the web bundle, or configure `VITE_RELAY_URL` to the **community's** `wss://` relay at build time (the relay's host binds tenant identity). The page uses an already-admitted NIP-07 browser signer. It never accepts a private key. Copy a channel UUID from desktop; the relay rechecks community membership and channel access for each request.

| Capability | Desktop | First phone-web slice | Gap |
| --- | --- | --- | --- |
| Identity | Local desktop identity / relay auth | Existing NIP-07 signer, signed NIP-98 HTTP requests and events | No phone pairing or mobile signer enrollment; most mobile browsers have no NIP-07 provider |
| Channels and messages | Channel browser, chat, history | None | No channel discovery or messages; channel UUID must come from desktop |
| Workflow list | Relay kind 30620, `#h` channel query | Same authorized relay `/query` and signed event content | One channel at a time; no channel overview |
| Create | Signed kind 30620, server parses YAML/validates membership and roles | YAML editor, signed kind 30620, relay validates | Webhook creation intentionally rejected because one-time secret is not handled; no edit/delete UI |
| Manual run | Signed kind 46020, owner authority checked by relay | Same event and relay owner check | Scheduled/webhook triggers remain server-side; no web controls for them |
| Status | Relay-owned workflow runs | Authorized `/workflows/{id}/runs` read and refresh | First 20 runs only; no pagination, approvals, or live stream |
| Install | Native desktop/mobile clients | Web manifest and icons, standalone start route | No deployed URL or real phone/account/agent test; no offline worker |

Security boundary: the relay remains authoritative for NIP-98 authentication, community/channel membership, event signatures, workflow schema, elevated `call_webhook` permissions, owner-only manual triggers, and read visibility. No private keys, webhook secrets, or relay bearer tokens are stored by this web slice. The starter YAML is a real manual `send_message` action; running it can post into the selected channel. The UI does not claim success until the relay accepts the event, but a successful trigger acceptance is not a successful run—refresh run history for its actual status.

Verification without a real account covers build/typecheck/lint, the existing invite smoke tests, a 390px Playwright browser with no signer (fail-closed), and a captured screenshot. **Not yet verified:** authorized relay create/run/status against a member account; phone NIP-07 availability; production deployment; agent execution. Do not call this user-ready or replace Telegram until those tests pass.
