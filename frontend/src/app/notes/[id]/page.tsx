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

export default function NotePage({ params }: { params: { id: string } }) {
  const noteId = params.id;
  const docRef = useRef<Y.Doc>();
  const [body, setBody] = useState("");
  const [title, setTitle] = useState("");
  const [savedTitle, setSavedTitle] = useState("");
  const [status, setStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");

  const [applyOps] = useMutation(APPLY_OPS);
  const [renameNote] = useMutation(RENAME_NOTE);
  const { data: notesData } = useQuery(MY_NOTES);

  if (!docRef.current) docRef.current = new Y.Doc();
  const doc = docRef.current;

  // Pick this note's title out of the cached list.
  useEffect(() => {
    const n = (notesData?.myNotes ?? []).find((x: any) => x.id === noteId);
    if (n) {
      setTitle(n.title);
      setSavedTitle(n.title);
    }
  }, [notesData, noteId]);

  useSubscription(NOTE_OPS, {
    variables: { noteId },
    onData: ({ data }) => {
      const upd = data.data?.noteOps?.updateB64;
      if (upd) {
        Y.applyUpdate(doc, b64decode(upd));
        setBody(doc.getText("body").toString());
      }
    },
  });

  useEffect(() => {
    const ytext = doc.getText("body");
    const obs = () => setBody(ytext.toString());
    ytext.observe(obs);
    return () => ytext.unobserve(obs);
  }, [doc]);

  const onBodyChange = async (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const next = e.target.value;
    const ytext = doc.getText("body");
    const before = Y.encodeStateVector(doc);
    doc.transact(() => {
      ytext.delete(0, ytext.length);
      ytext.insert(0, next);
    });
    const upd = Y.encodeStateAsUpdate(doc, before);
    setStatus("saving");
    try {
      await applyOps({ variables: { noteId, updateB64: b64encode(upd) } });
      setStatus("saved");
    } catch {
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
