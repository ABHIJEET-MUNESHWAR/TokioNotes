import { ApolloClient, InMemoryCache, HttpLink, split, ApolloLink } from "@apollo/client";
import { GraphQLWsLink } from "@apollo/client/link/subscriptions";
import { getMainDefinition } from "@apollo/client/utilities";
import { createClient } from "graphql-ws";

const HTTP_URL = process.env.NEXT_PUBLIC_GRAPHQL_HTTP ?? "http://localhost:9090/graphql";
const WS_URL = process.env.NEXT_PUBLIC_GRAPHQL_WS ?? "ws://localhost:9090/graphql";

function authLink(): ApolloLink {
  return new ApolloLink((operation, forward) => {
    const token = typeof window !== "undefined" ? localStorage.getItem("tn_token") : null;
    if (token) {
      operation.setContext(({ headers = {} }: any) => ({
        headers: { ...headers, authorization: `Bearer ${token}` },
      }));
    }
    return forward(operation);
  });
}

export function makeClient() {
  const http = new HttpLink({ uri: HTTP_URL });
  const ws = typeof window !== "undefined"
    ? new GraphQLWsLink(createClient({
        url: WS_URL,
        // Re-evaluated on every (re)connect, so a fresh token after login
        // is automatically picked up on the next reconnection.
        connectionParams: () => {
          const token = localStorage.getItem("tn_token");
          return token ? { authorization: `Bearer ${token}` } : {};
        },
        // Try to keep the connection alive — closes will be retried with
        // exponential backoff by graphql-ws.
        retryAttempts: Infinity,
        shouldRetry: () => true,
      }))
    : null;
  const link = ws
    ? split(
        ({ query }) => {
          const def = getMainDefinition(query);
          return def.kind === "OperationDefinition" && def.operation === "subscription";
        },
        ws,
        authLink().concat(http),
      )
    : authLink().concat(http);
  return new ApolloClient({ link, cache: new InMemoryCache() });
}

