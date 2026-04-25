import Link from "next/link";

export const metadata = { title: "Not found · TokioNotes" };

export default function NotFound() {
  return (
    <div className="tn-container">
      <div
        className="tn-card tn-stack tn-not-found"
        style={{ maxWidth: 520, margin: "60px auto", textAlign: "center" }}
      >
        <div className="tn-404-code" aria-hidden>
          404
        </div>
        <h2 style={{ margin: 0 }}>Page not found</h2>
        <p className="tn-muted" style={{ margin: 0 }}>
          The page you’re looking for doesn’t exist, was moved, or you may not
          have access to it. If you followed a note link, the note may have
          been deleted or its sharing was revoked.
        </p>
        <div className="tn-row" style={{ justifyContent: "center" }}>
          <Link href="/" className="tn-btn tn-btn-primary">
            ← Back to my notes
          </Link>
        </div>
      </div>
    </div>
  );
}

