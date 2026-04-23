"use client";
import { ApolloProvider } from "@apollo/client";
import { useMemo } from "react";
import { makeClient } from "./apollo";

export function Providers({ children }: { children: React.ReactNode }) {
  const client = useMemo(() => makeClient(), []);
  return <ApolloProvider client={client}>{children}</ApolloProvider>;
}

