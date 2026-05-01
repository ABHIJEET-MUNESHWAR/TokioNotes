"use client";
import { useMutation, useQuery, useSubscription } from "@apollo/client";
import { notFound } from "next/navigation";
import { useCallback, useEffect, useRef, useState } from "react";
import * as Y from "yjs";
import {
  APPLY_OPS,
  COLLABORATORS,
  MY_NOTES,
  NOTE_OPS,
  RENAME_NOTE,
  REVOKE_SHARE,
  SHARE_NOTE,
} from "@/lib/queries";

// ----- CRDT-send debounce knobs --------------------------------------------
//
// Each keystroke produces a tiny Yjs update, but the *visible* effect on
// other tabs only needs to land within ~half a second to feel "live".
// We buffer locally and send one cumulative update covering everything
// since the last successful send. Quiet pauses (>QUIET_MS) flush
// immediately; continuous typing flushes at most every MAX_MS.
const SEND_QUIET_MS = 150;
const SEND_MAX_MS = 600;

function b64encode(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}
function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Minimal common-prefix / common-suffix diff for plain text. */
function diffEdit(prev: string, next: string): { index: number; remove: number; insert: string } {
  let start = 0;
  const minLen = Math.min(prev.length, next.length);
  while (start < minLen && prev.charCodeAt(start) === next.charCodeAt(start)) start++;
  let endPrev = prev.length;
  let endNext = next.length;
  while (
    endPrev > start &&
    endNext > start &&
    prev.charCodeAt(endPrev - 1) === next.charCodeAt(endNext - 1)
  ) {
    endPrev--;
    endNext--;
  }
  return { index: start, remove: endPrev - start, insert: next.slice(start, endNext) };
}

export default function NotePage({ params }: { params: { id: string } }) {
  const noteId = params.id;
  const docRef = useRef<Y.Doc>();
  const seededRef = useRef(false);
  const [body, setBody] = useState("");
  const [title, setTitle] = useState("");
  const [savedTitle, setSavedTitle] = useState("");
  const [myRole, setMyRole] = useState<"VIEWER" | "EDITOR" | "OWNER" | null>(null);
  const [status, setStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");

  const canEdit = myRole === "EDITOR" || myRole === "OWNER";

  const [applyOps] = useMutation(APPLY_OPS);
  const [renameNote] = useMutation(RENAME_NOTE);
  const { data: notesData, loading: notesLoading } = useQuery(MY_NOTES, {
    fetchPolicy: "cache-and-network",
  });

  if (!docRef.current) docRef.current = new Y.Doc();
  const doc = docRef.current;

  // -------- Send-side debouncer state --------------------------------------
  // `lastSentSV` is the Yjs state vector at the moment of our last
  // successful applyOps; the cumulative update we send next is exactly
  // the diff from that vector to "now". So no matter how many keystrokes
  // we coalesce, the receiver merges them in a single CRDT apply.
  const lastSentSVRef = useRef<Uint8Array | null>(null);
  const quietTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hardTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dirtyRef = useRef(false);
  const inFlightRef = useRef(false);

  // If we've finished loading the user's notes and the requested id isn't
  // among them (deleted, share revoked, or never existed) hand off to the
  // global 404 page.
  if (!notesLoading && notesData) {
    const exists = (notesData.myNotes ?? []).some((n: any) => n.id === noteId);
    if (!exists) notFound();
  }

  // 1. Seed the local Y.Doc from the server snapshot exactly once.
  // 2. Track the title for the renameNote mutation.
  useEffect(() => {
    const n = (notesData?.myNotes ?? []).find((x: any) => x.id === noteId);
    if (!n) return;
    setTitle(n.title);
    setSavedTitle(n.title);
    if (n.myRole) setMyRole(n.myRole);
    if (!seededRef.current && n.snapshotB64) {
      try {
        Y.applyUpdate(doc, b64decode(n.snapshotB64));
        setBody(doc.getText("body").toString());
      } catch (e) {
        console.warn("failed to apply snapshot", e);
      }
      // After seeding, our "last sent" baseline is the doc itself —
      // any future edits will be diffed against this.
      lastSentSVRef.current = Y.encodeStateVector(doc);
      seededRef.current = true;
    } else if (!seededRef.current) {
      // No snapshot but we've still observed the note → treat as seeded.
      lastSentSVRef.current = Y.encodeStateVector(doc);
      seededRef.current = true;
    }
  }, [notesData, noteId, doc]);

  // Live updates from collaborators (and echoes of our own writes).
  useSubscription(NOTE_OPS, {
    variables: { noteId },
    onData: ({ data }) => {
      const upd = data.data?.noteOps?.updateB64;
      if (!upd) return;
      Y.applyUpdate(doc, b64decode(upd));
      setBody(doc.getText("body").toString());
    },
    onError: (err) => {
      // Surface WS auth/routing problems instead of failing silently.
      // eslint-disable-next-line no-console
      console.error("noteOps subscription error", err);
    },
  });

  // Mirror Yjs body changes (from any source) into local React state.
  useEffect(() => {
    const ytext = doc.getText("body");
    const obs = () => setBody(ytext.toString());
    ytext.observe(obs);
    return () => ytext.unobserve(obs);
  }, [doc]);

  /** Send one cumulative applyOps for everything since `lastSentSV`. */
  const flushSend = useCallback(async () => {
    if (!canEdit) return;
    if (inFlightRef.current) return; // a flush is already on the wire
    if (!dirtyRef.current) return;
    if (quietTimerRef.current) {
      clearTimeout(quietTimerRef.current);
      quietTimerRef.current = null;
    }
    if (hardTimerRef.current) {
      clearTimeout(hardTimerRef.current);
      hardTimerRef.current = null;
    }
    const sv = lastSentSVRef.current ?? new Uint8Array();
    const upd = Y.encodeStateAsUpdate(doc, sv);
    if (upd.length === 0) {
      dirtyRef.current = false;
      return;
    }
    inFlightRef.current = true;
    setStatus("saving");
    try {
      await applyOps({ variables: { noteId, updateB64: b64encode(upd) } });
      // Advance the baseline only on success. If the call fails we keep
      // the old SV so the next flush retries the same range plus any
      // new edits that arrived in the meantime — at-least-once semantics
      // are safe because Yjs updates are idempotent.
      lastSentSVRef.current = Y.encodeStateVector(doc);
      dirtyRef.current = false;
      setStatus("saved");
    } catch (err) {
      console.error("applyOps failed", err);
      setStatus("error");
    } finally {
      inFlightRef.current = false;
      // If more edits arrived while in flight, schedule another quiet
      // flush so the trailing burst doesn't wait for the next keystroke.
      if (dirtyRef.current && !quietTimerRef.current) {
        quietTimerRef.current = setTimeout(() => {
          quietTimerRef.current = null;
          flushSend();
        }, SEND_QUIET_MS);
      }
    }
  }, [applyOps, canEdit, doc, noteId]);

  /** Mark dirty and (re)arm the debounce timers. */
  const scheduleSend = useCallback(() => {
    dirtyRef.current = true;
    if (quietTimerRef.current) clearTimeout(quietTimerRef.current);
    quietTimerRef.current = setTimeout(() => {
      quietTimerRef.current = null;
      flushSend();
    }, SEND_QUIET_MS);
    if (!hardTimerRef.current) {
      hardTimerRef.current = setTimeout(() => {
        hardTimerRef.current = null;
        flushSend();
      }, SEND_MAX_MS);
    }
  }, [flushSend]);

  // Flush on unmount so navigating away never loses buffered edits.
  useEffect(() => {
    return () => {
      if (dirtyRef.current) {
        // Best-effort fire-and-forget; the page is going away anyway.
        flushSend();
      }
      if (quietTimerRef.current) clearTimeout(quietTimerRef.current);
      if (hardTimerRef.current) clearTimeout(hardTimerRef.current);
    };
  }, [flushSend]);

  const onBodyChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    if (!canEdit) return;
    const next = e.target.value;
    const ytext = doc.getText("body");
    const prev = ytext.toString();
    if (prev === next) return;

    // Compute a minimal CRDT edit instead of replacing the whole text —
    // this preserves intent and merges cleanly with concurrent edits.
    const { index, remove, insert } = diffEdit(prev, next);
    doc.transact(() => {
      if (remove > 0) ytext.delete(index, remove);
      if (insert.length > 0) ytext.insert(index, insert);
    }, "local");

    // Don't send yet — the debouncer will batch a burst of keystrokes
    // into a single applyOps call. Local Yjs state already advanced, so
    // the textarea reflects the change immediately via the observer.
    scheduleSend();
  };

  const commitTitle = async () => {
    if (!canEdit) return;
    const next = title.trim();
    if (!next || next === savedTitle) return;
    // Make sure any pending body edits land before the rename so users
    // never see a stale snapshot in `myNotes`.
    if (dirtyRef.current) await flushSend();
    setStatus("saving");
    try {
      await renameNote({ variables: { id: noteId, title: next } });
      setSavedTitle(next);
      setStatus("saved");
    } catch {
      setStatus("error");
    }
  };

  return (
    <div className="tn-container">
      <div className="tn-card tn-stack">
        <div className="tn-row" style={{ justifyContent: "space-between" }}>
          <a href="/" className="tn-muted">← Back to notes</a>
          <span className="tn-muted">
            {myRole && (
              <span style={{ marginRight: 12 }}>
                Role: <strong>{myRole}</strong>
                {!canEdit && " (read-only)"}
              </span>
            )}
            {status === "saving" && "Saving…"}
            {status === "saved" && "Saved ✓"}
            {status === "error" && "Save failed"}
          </span>
        </div>

        <div>
          <span className="tn-section-label">Title</span>
          <input
            className="tn-title-input"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={commitTitle}
            onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }}
            placeholder="Untitled note"
            readOnly={!canEdit}
          />
        </div>

        <div>
          <span className="tn-section-label">Body</span>
          <textarea
            className="tn-textarea"
            value={body}
            onChange={onBodyChange}
            onBlur={() => {
              if (dirtyRef.current) flushSend();
            }}
            placeholder={canEdit
              ? "Start writing — changes sync live to every collaborator…"
              : "You have view-only access to this note."}
            readOnly={!canEdit}
          />
        </div>

        <p className="tn-muted" style={{ margin: 0 }}>
          Note ID: <code>{noteId}</code>
        </p>
      </div>

      <SharePanel noteId={noteId} />
    </div>
  );
}

function SharePanel({ noteId }: { noteId: string }) {
  const { data, loading, refetch } = useQuery(COLLABORATORS, {
    variables: { id: noteId },
    fetchPolicy: "cache-and-network",
  });
  const [shareNote] = useMutation(SHARE_NOTE);
  const [revokeShare] = useMutation(REVOKE_SHARE);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"VIEWER" | "EDITOR" | "OWNER">("EDITOR");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const target = email.trim();
    if (!target) return;
    setError(null);
    setBusy(true);
    try {
      await shareNote({ variables: { id: noteId, email: target, role } });
      setEmail("");
      await refetch();
    } catch (e: any) {
      setError(e.message ?? "Share failed");
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (userId: string) => {
    setError(null);
    try {
      await revokeShare({ variables: { id: noteId, userId } });
      await refetch();
    } catch (e: any) {
      setError(e.message ?? "Revoke failed");
    }
  };

  const collaborators: Array<{
    userId: string;
    role: string;
    displayName: string;
    email: string;
  }> = data?.collaborators ?? [];

  return (
    <div className="tn-card tn-stack" style={{ marginTop: 16 }}>
      <h3 style={{ margin: 0 }}>Share & collaborate</h3>
      <p className="tn-muted" style={{ margin: 0 }}>
        Invite another registered user by email. Editors can co-edit the body
        live; viewers only see updates. Only owners can share or revoke access.
      </p>

      <div className="tn-row" style={{ flexWrap: "wrap", gap: 8 }}>
        <input
          className="tn-input"
          style={{ flex: 1, minWidth: 220 }}
          placeholder="collaborator@example.com"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
        />
        <select
          className="tn-input"
          value={role}
          onChange={(e) => setRole(e.target.value as any)}
          style={{ minWidth: 140 }}
        >
          <option value="VIEWER">Viewer</option>
          <option value="EDITOR">Editor</option>
          <option value="OWNER">Owner</option>
        </select>
        <button
          className="tn-btn tn-btn-primary"
          onClick={submit}
          disabled={busy || !email.trim()}
        >
          {busy ? "Sharing…" : "Share"}
        </button>
      </div>
      {error && <p style={{ color: "var(--danger)", margin: 0 }}>{error}</p>}

      <div>
        <span className="tn-section-label">Collaborators</span>
        {loading && !collaborators.length && (
          <p className="tn-muted">Loading…</p>
        )}
        {!loading && collaborators.length === 0 && (
          <p className="tn-muted">No collaborators yet.</p>
        )}
        <ul className="tn-list">
          {collaborators.map((c) => {
            const name = c.displayName || c.email || c.userId.slice(0, 8) + "…";
            return (
              <li
                key={c.userId}
                className="tn-row"
                style={{ justifyContent: "space-between" }}
              >
                <span style={{ display: "flex", flexDirection: "column" }}>
                  <strong>{name}</strong>
                  {c.email && (
                    <span className="tn-meta">{c.email}</span>
                  )}
                  <span className="tn-meta">{c.role}</span>
                </span>
                {c.role !== "OWNER" && (
                  <button
                    className="tn-btn"
                    onClick={() => revoke(c.userId)}
                    title="Revoke access"
                  >
                    Revoke
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
