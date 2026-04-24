"use client";
import { useMutation, useQuery } from "@apollo/client";
import { useEffect, useState } from "react";
import { CREATE_NOTE, LOGIN, MY_NOTES, REGISTER } from "@/lib/queries";

function AuthBox({ onAuth }: { onAuth: () => void }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [register] = useMutation(REGISTER);
  const [login] = useMutation(LOGIN);

  const handle = async (mode: "login" | "register") => {
    setError(null);
    try {
      const res =
        mode === "register"
          ? await register({
              variables: { email, displayName: name || email, password },
            })
          : await login({ variables: { email, password } });
      const token = (res.data as any)[mode].token;
      localStorage.setItem("tn_token", token);
      onAuth();
    } catch (e: any) {
      setError(e.message ?? "Authentication failed");
    }
  };

  return (
    <div className="tn-container">
      <div
        className="tn-card tn-stack"
        style={{ maxWidth: 420, margin: "40px auto" }}
      >
        <h2 style={{ margin: 0 }}>Sign in / Register</h2>
        <p className="tn-muted" style={{ margin: 0 }}>
          Welcome to TokioNotes — collaborative notes powered by Rust + CRDT.
        </p>
        <input
          className="tn-input"
          placeholder="email"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
        <input
          className="tn-input"
          placeholder="display name (register only)"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="tn-input"
          placeholder="password"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <div className="tn-row">
          <button
            className="tn-btn tn-btn-primary"
            onClick={() => handle("login")}
          >
            Login
          </button>
          <button className="tn-btn" onClick={() => handle("register")}>
            Register
          </button>
        </div>
        {error && (
          <p style={{ color: "var(--danger)", margin: 0 }}>{error}</p>
        )}
      </div>
    </div>
  );
}

function NotesList() {
  const { data, loading, refetch } = useQuery(MY_NOTES);
  const [createNote] = useMutation(CREATE_NOTE);
  const [title, setTitle] = useState("");

  const create = async () => {
    if (!title.trim()) return;
    await createNote({ variables: { title: title.trim() } });
    setTitle("");
    refetch();
  };

  return (
    <div className="tn-container tn-stack">
      <div className="tn-card tn-stack">
        <h2 style={{ margin: 0 }}>Create note</h2>
        <span className="tn-section-label">Title</span>
        <div className="tn-row">
          <input
            className="tn-input"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") create();
            }}
            placeholder="e.g. Project ideas"
            style={{ flex: 1, minWidth: 240 }}
          />
          <button className="tn-btn tn-btn-primary" onClick={create}>
            Create
          </button>
        </div>
      </div>

      <div className="tn-card tn-stack">
        <h2 style={{ margin: 0 }}>My notes</h2>
        {loading && <p className="tn-muted">Loading…</p>}
        {!loading && (data?.myNotes ?? []).length === 0 && (
          <p className="tn-muted">No notes yet — create your first one above.</p>
        )}
        <ul className="tn-list">
          {(data?.myNotes ?? []).map((n: any) => (
            <li key={n.id}>
              <a
                href={`/notes/${n.id}`}
                style={{ fontWeight: 600 }}
              >
                {n.title}
              </a>
              <span className="tn-meta">
                {n.updatedAt
                  ? new Date(n.updatedAt).toLocaleString()
                  : ""}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

export default function Home() {
  // Start with a deterministic value so server- and client-rendered HTML match,
  // then read the persisted token after mount to avoid a hydration mismatch.
  const [hydrated, setHydrated] = useState(false);
  const [authed, setAuthed] = useState(false);

  useEffect(() => {
    setAuthed(!!localStorage.getItem("tn_token"));
    setHydrated(true);
  }, []);

  if (!hydrated) {
    return (
      <div className="tn-container">
        <p className="tn-muted">Loading…</p>
      </div>
    );
  }
  return authed ? <NotesList /> : <AuthBox onAuth={() => setAuthed(true)} />;
}
