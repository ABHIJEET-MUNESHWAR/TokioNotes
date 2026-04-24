import "./globals.css";
import { Providers } from "@/lib/Providers";
import { ThemeProvider, ThemeToggle } from "@/lib/theme";

export const metadata = { title: "TokioNotes" };

// Set the theme attribute before React hydrates to avoid a flash of the wrong theme.
const themeBootScript = `
  (function () {
    try {
      var stored = localStorage.getItem('tn_theme');
      var theme = stored === 'light' || stored === 'dark'
        ? stored
        : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
      document.documentElement.setAttribute('data-theme', theme);
    } catch (_) {}
  })();
`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
      </head>
      <body>
        <ThemeProvider>
          <Providers>
            <header className="tn-header">
              <h1><a href="/">📝 TokioNotes</a></h1>
              <ThemeToggle />
            </header>
            <main>{children}</main>
          </Providers>
        </ThemeProvider>
      </body>
    </html>
  );
}
