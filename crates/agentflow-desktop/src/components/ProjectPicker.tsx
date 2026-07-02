import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import type { ProjectOverview } from "../types";

interface ProjectPickerProps {
  onOpened: (overview: ProjectOverview) => void;
  onError: (message: string) => void;
}

export default function ProjectPicker({ onOpened, onError }: ProjectPickerProps) {
  async function pickAndOpen() {
    const path = await open({ directory: true, multiple: false });
    if (!path || Array.isArray(path)) {
      return;
    }
    try {
      const overview = await invoke<ProjectOverview>("open_project", { path });
      onOpened(overview);
    } catch (error) {
      onError(String(error));
    }
  }

  return (
    <section className="project-picker" aria-labelledby="project-picker-title">
      <div className="project-picker-visual" aria-hidden="true">
        <span className="visual-panel visual-panel-primary" />
        <span className="visual-panel visual-panel-secondary" />
        <span className="visual-rail" />
      </div>

      <div className="project-picker-copy">
        <p className="eyebrow">Workspace</p>
        <h2 id="project-picker-title">Open an AgentFlow project</h2>
        <p>
          Select a local project directory to inspect its engine version,
          timestamps, and current object counts.
        </p>
      </div>

      <button className="primary-button" type="button" onClick={pickAndOpen}>
        Open Project&hellip;
      </button>
      <p className="project-picker-note">Project data stays on this device.</p>
    </section>
  );
}
