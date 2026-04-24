"use client";
import { useApolloClient, useQuery } from "@apollo/client";
import { useEffect, useState } from "react";
import { ME } from "@/lib/queries";

/**
 * Header widget: shows the signed-in user's display name + email and a
 * Logout button. Hidden until we've confirmed (client-side) that a token
 * is present, so the SSR HTML matches the first client render.
 */
export function UserMenu() {
  const [hydrated, setHydrated] = useState(false);
  const [hasToken, setHasToken] = useState(false);
  const client = useApolloClient();

  useEffect(() => {
    setHasToken(!!localStorage.getItem("tn_token"));
    setHydrated(true);
    // Re-evaluate when another tab logs in/out.
    const onStorage = (e: StorageEvent) => {
      if (e.key === "tn_token") setHasToken(!!e.newValue);
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const { data, loading } = useQuery(ME, {
    skip: !hydrated || !hasToken,
    fetchPolicy: "cache-and-network",
  });

  if (!hydrated || !hasToken) return null;

  const me = data?.me;
  const display = me?.displayName || me?.email || "…";
  const email = me?.email || "";
  const initial = (display[0] || "?").toUpperCase();

  const logout = async () => {
    localStorage.removeItem("tn_token");
    try {
      await client.clearStore();
    } catch {
      /* ignore */
    }
    // Hard redirect so any in-memory subscription clients are torn down.
    window.location.href = "/";
  };

  return (
    <div className="tn-user-menu">
      <div className="tn-avatar" aria-hidden>{initial}</div>
      <div className="tn-user-meta">
        <span className="tn-user-name">{loading && !me ? "Loading…" : display}</span>
        {email && <span className="tn-user-email">{email}</span>}
      </div>
      <button className="tn-btn" onClick={logout} title="Sign out">
        Logout
      </button>
    </div>
  );
}

