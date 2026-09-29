import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

// Scaffold of step 11.1: it proves the chain (Vite build, embedded in the binary, served
// under the secret path, API reachable). The real app comes in step 11.3, from the
// prototype in design/prototype.
function App() {
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    fetch("./api/version")
      .then((r) => r.json())
      .then((d: { version: string }) => setVersion(d.version))
      .catch(() => setVersion(null));
  }, []);

  return (
    <main style={{ display: "grid", placeItems: "center", height: "100vh", textAlign: "center" }}>
      <div>
        <h1 style={{ margin: 0, color: "#e9c46a", letterSpacing: ".06em" }}>Kariz</h1>
        <p style={{ color: "#a9b8cc" }}>{version ? `panel ${version}` : "…"}</p>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
