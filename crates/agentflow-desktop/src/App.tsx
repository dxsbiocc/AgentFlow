import { useState } from "react";
import ProjectPicker from "./components/ProjectPicker";
import ProjectOverview from "./components/ProjectOverview";
import type { ProjectOverview as ProjectOverviewDto } from "./types";
import "./App.css";

function App() {
  const [project, setProject] = useState<ProjectOverviewDto | null>(null);
  const [error, setError] = useState<string | null>(null);

  return (
    <main className="app-shell">
      <section className="app-frame" aria-labelledby="app-title">
        <header className="app-header">
          <div>
            <p className="eyebrow">AgentFlow Desktop</p>
            <h1 id="app-title">{project ? "Project status" : "Open a project"}</h1>
          </div>
          <span className="app-badge">{project ? "Read-only" : "Local"}</span>
        </header>

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        <div className="app-content">
          {project ? (
            <ProjectOverview
              overview={project}
              onClose={() => {
                setProject(null);
                setError(null);
              }}
            />
          ) : (
            <ProjectPicker
              onOpened={(overview) => {
                setProject(overview);
                setError(null);
              }}
              onError={setError}
            />
          )}
        </div>
      </section>
    </main>
  );
}

export default App;
