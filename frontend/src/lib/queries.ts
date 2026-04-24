import { gql } from "@apollo/client";

export const REGISTER = gql`
  mutation Register($email: String!, $displayName: String!, $password: String!) {
    register(email: $email, displayName: $displayName, password: $password) {
      token
      user { id email displayName }
    }
  }
`;

export const LOGIN = gql`
  mutation Login($email: String!, $password: String!) {
    login(email: $email, password: $password) {
      token
      user { id email displayName }
    }
  }
`;

export const MY_NOTES = gql`
  query MyNotes {
    myNotes { id title updatedAt snapshotB64 }
  }
`;

export const CREATE_NOTE = gql`
  mutation CreateNote($title: String!) {
    createNote(title: $title) { id title snapshotB64 }
  }
`;

export const RENAME_NOTE = gql`
  mutation RenameNote($id: NoteId!, $title: String!) {
    renameNote(id: $id, title: $title) { id title updatedAt }
  }
`;

export const SHARE_NOTE = gql`
  mutation ShareNote($id: NoteId!, $email: String!, $role: Role!) {
    shareNote(id: $id, email: $email, role: $role) { userId role }
  }
`;

export const APPLY_OPS = gql`
  mutation ApplyOps($noteId: NoteId!, $updateB64: String!) {
    applyOps(noteId: $noteId, updateB64: $updateB64)
  }
`;

export const NOTE_OPS = gql`
  subscription NoteOps($noteId: NoteId!) {
    noteOps(noteId: $noteId) { noteId updateB64 }
  }
`;

export const AI_SUMMARY = gql`
  query AiSummary($id: NoteId!) { aiSummary(id: $id) }
`;
