"use client";
import { useMutation, useSubscription } from "@apollo/client";
import { useEffect, useRef, useState } from "react";
import * as Y from "yjs";
import { APPLY_OPS, NOTE_OPS } from "@/lib/queries";

function b64encode(bytes: Uint8Array): string {
  let s = ""; for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}
function b64decode(s: string): Uint8Array {
  const bin = atob(s); const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export default function NotePage({ params }: { params: { id: string } }) {
  const noteId = params.id;
  const docRef = useRef<Y.Doc>();
  const [text, setText] = useState("");
  const [applyOps] = useMutation(APPLY_OPS);

  if (!docRef.current) docRef.current = new Y.Doc();
  const doc = docRef.current;

  useSubscription(NOTE_OPS, {
    variables: { noteId },
    onData: ({ data }) => {
      const upd = data.data?.noteOps?.updateB64;
      if (upd) {
        Y.applyUpdate(doc, b64decode(upd));
        setText(doc.getText("body").toString());
      }
    },
  });

  useEffect(() => {
    const ytext = doc.getText("body");
    const obs = () => setText(ytext.toString());
    ytext.observe(obs);
    return () => ytext.unobserve(obs);
  }, [doc]);

  const onChange = async (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const next = e.target.value;
    const ytext = doc.getText("body");
    const before = Y.encodeStateVector(doc);
    doc.transact(() => {
      ytext.delete(0, ytext.length);
      ytext.insert(0, next);
    });
    const upd = Y.encodeStateAsUpdate(doc, before);
    await applyOps({ variables: { noteId, updateB64: b64encode(upd) } });
  };

  return (
    <div style={{ padding: 16 }}>
      <h2>Note {noteId}</h2>
      <textarea value={text} onChange={onChange} rows={20} cols={80} />
    </div>
  );
}

