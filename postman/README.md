# Postman

Import `TokioNotes.postman_collection.json` into Postman
(*Import → File*). The collection ships with:

- Collection variables — `baseUrl` (default `http://localhost:9090`),
  `wsUrl` (`ws://localhost:9090/graphql`), `token`, `tokenB`, `noteId`,
  `userId`, plus pre-seeded credentials for Alice & Bob and a
  `sampleUpdateB64` Y-CRDT update.
- **Auto-captured auth** — `Auth → Register (Alice)` and
  `Auth → Login (Alice)` run a test script that stores the returned JWT
  into `{{token}}`. `Register (Bob)` captures `{{tokenB}}` so you can
  test shared-note access as a second user.
- **Auto-captured IDs** — `Notes → Create Note` stores the new
  `{{noteId}}`; `Sharing → Share Note with Bob` stores the target
  `{{userId}}`.

## Suggested run order

1. `Health`
2. `Auth → Register (Alice)` → `Auth → Register (Bob)`
3. `Notes → Create Note` → `Notes → My Notes` → `Notes → Get Note`
4. `Sharing → Share Note with Bob (Editor)` → `Sharing → List Collaborators`
5. `Sharing → Bob: My Notes (uses tokenB)` — verify Bob sees the shared note
6. `Collaboration → Apply Ops (sample update)`
7. `AI → AI Summary`
8. Optional: `Notes → Rename Note`, `Notes → Delete Note`,
   `Sharing → Revoke Share`.

## Real-time subscription

`Collaboration → Note Ops (Subscription, WebSocket)` is a WebSocket
request to `{{wsUrl}}` using the `graphql-transport-ws` sub-protocol.
Open it, press **Connect**, then send the two messages documented in the
request description (`connection_init` → `subscribe`). Fire the
`Apply Ops` mutation in another tab to observe live `next` frames.

## Generating fresh CRDT updates

The bundled `sampleUpdateB64` is only valid as the very first edit on an
empty document. For further edits, generate updates from the Next.js
frontend (`frontend/src/app/notes/[id]/page.tsx` uses
`Y.encodeStateAsUpdate(doc, stateVector)`) or any Yjs client, base64-encode
them, and paste into the `u` variable of the `Apply Ops` request.

