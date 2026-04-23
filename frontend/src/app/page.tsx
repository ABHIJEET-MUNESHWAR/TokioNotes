"use client";
import { useMutation, useQuery } from "@apollo/client";
import { useState } from "react";
import { CREATE_NOTE, LOGIN, MY_NOTES, REGISTER } from "@/lib/queries";

function AuthBox({ onAuth }: { onAuth: () => void }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [register] = useMutation(REGISTER);
  const [login] = useMutation(LOGIN);

  const handle = async (mode: "login" | "register") => {
    const res = mode === "register"
      ? await register({ variables: { email, displayName: name || email, password } })
      : await login({ variables: { email, password } });
    const token = (res.data as any)[mode].token;
    localStorage.setItem("tn_token", token);
    onAuth();
  };

  return (
    <div style={{ padding: 16, maxWidth: 360 }}>
      <h2>Sign in / Register</h2>
      <input placeholder="email" value={email} onChange={e => setEmail(e.target.value)} /><br />
      <input placeholder="display name (register)" value={name} onChange={e => setName(e.target.value)} /><br />
      <input placeholder="password" type="password" value={password} onChange={e => setPassword(e.target.value)} /><br />
      <button onClick={() => handle("login")}>Login</button>
      <button onClick={() => handle("register")}>Register</button>
    </div>
  );
}

function NotesList() {
  const { data, loading, refetch } = useQuery(MY_NOTES);
  const [createNote] = useMutation(CREATE_NOTE);
  const [title, setTitle] = useState("");
  if (loading) return <p>Loading…</p>;
  return (
    <div style={{ padding: 16 }}>
      <h2>My notes</h2>
      <input value={title} onChange={e => setTitle(e.target.value)} placeholder="title" />
      <button onClick={async () => { await createNote({ variables: { title } }); setTitle(""); refetch(); }}>
        Create
      </button>
      <ul>
        {(data?.myNotes ?? []).map((n: any) => (
          <li key={n.id}><a href={`/notes/${n.id}`}>{n.title}</a></li>
        ))}
      </ul>
    </div>
  );
}

export default function Home() {
  const [authed, setAuthed] = useState(typeof window !== "undefined" && !!localStorage.getItem("tn_token"));
  return authed ? <NotesList /> : <AuthBox onAuth={() => setAuthed(true)} />;
}

