"use client";
import { useMutation, useQuery, useSubscription } from "@apollo/client";
import { useEffect, useRef, useState } from "react";
import * as Y from "yjs";
import { APPLY_OPS, MY_NOTES, NOTE_OPS, RENAME_NOTE } from "@/lib/queries";

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
  const [status, setStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");

  const [applyOps] = useMutation(APPLY_OPS);
  const [renameNote] = useMutation(RENAME_NOTE);
  const { data: notesData } = useQuery(MY_NOTES, { fetchPolicy: "cache-and-network" });

  if (!docRef.current) docRef.current = new Y.Doc();
  const doc = docRef.current;

  // 1. Seed the local Y.Doc from the server snapshot exactly once.
  // 2. Track the title for the renameNote mutation.
  useEffect(() => {
    const n = (notesData?.myNotes ?? []).find((x: any) => x.id === noteId);
    if (!n) return;
    setTitle(n.title);
    setSavedTitle(n.title);
    if (!seededRef.current && n.snapshotB64) {
      try {
        Y.applyUpdate(doc, b64decode(n.snapshotB64));
        setBody(doc.getText("body").toString());
      } catch (e) {
        console.warn("failed to apply snapshot", e);
      }
      seededRef.current = true;
    } else if (!seededRef.current) {
      // No snapshot but we've still observed the note → treat as seeded.
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
  });

  // Mirror Yjs body changes (from any source) into local React state.
  useEffect(() => {
    const ytext = doc.getText("body");
    const obs = () => setBody(ytext.toString());
    ytext.observe(obs);
    return () => ytext.unobserve(obs);
  }, [doc]);

  const onBodyChange = async (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const next = e.target.value;
    const ytext = doc.getText("body");
    const prev = ytext.toString();
    if (prev === next) return;

    // Compute a minimal CRDT edit instead of replacing the whole text —
    // this preserves intent and merges cleanly with concurrent edits.
    const { index, remove, insert } = diffEdit(prev, next);
    const before = Y.encodeStateVector(doc);
    doc.transact(() => {
      if (remove > 0) ytext.delete(index, remove);
      if (insert.length > 0) ytext.insert(index, insert);
    }, "local");
    const upd = Y.encodeStateAsUpdate(doc, before);
    if (upd.length === 0) return;

    setStatus("saving");
    try {
      await applyOps({ variables: { noteId, updateB64: b64encode(upd) } });
      setStatus("saved");
    } catch (err) {
      console.error("applyOps failed", err);
      setStatus("error");
    }
  };

  const commitTitle = async () => {
    const next = title.trim();
    if (!next || next === savedTitle) return;
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
          />
        </div>

        <div>
          <span className="tn-section-label">Body</span>
          <textarea
            className="tn-textarea"
            value={body}
            onChange={onBodyChange}
            placeholder="Start writing — changes sync live to every collaborator…"
          />
        </div>

        <p className="tn-muted" style={{ margin: 0 }}>
          Note ID: <code>{noteId}</code>
        </p>
      </div>
    </div>
  );
}
