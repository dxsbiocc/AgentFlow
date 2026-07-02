import type { ProjectOverview as ProjectOverviewDto } from "../types";

interface ProjectOverviewProps {
  overview: ProjectOverviewDto;
  onClose: () => void;
}

function formatTimestamp(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString();
}

export default function ProjectOverview({ overview, onClose }: ProjectOverviewProps) {
  const { summary } = overview;

  const stats = [
    { label: "Flows", value: overview.flow_count },
    { label: "Tools", value: overview.tool_count },
    { label: "Runs", value: overview.run_count },
    { label: "Artifacts", value: overview.artifact_count },
  ];

  return (
    <section className="project-overview" aria-labelledby="project-overview-title">
      <header className="project-overview-header">
        <div>
          <p className="eyebrow">Project overview</p>
          <h2 id="project-overview-title">{summary.name}</h2>
        </div>
        <button className="secondary-button" type="button" onClick={onClose}>
          Close project
        </button>
      </header>

      <div className="project-overview-grid">
        <section className="metadata-panel" aria-labelledby="metadata-title">
          <h3 id="metadata-title">Metadata</h3>
          <dl className="project-overview-meta">
            <div className="meta-row">
              <dt>Root path</dt>
              <dd>{summary.root_path}</dd>
            </div>
            <div className="meta-row">
              <dt>Engine version</dt>
              <dd>{summary.engine_version}</dd>
            </div>
            <div className="meta-row">
              <dt>Created</dt>
              <dd>{formatTimestamp(summary.created_at)}</dd>
            </div>
            <div className="meta-row">
              <dt>Updated</dt>
              <dd>{formatTimestamp(summary.updated_at)}</dd>
            </div>
          </dl>
        </section>

        <section className="stats-panel" aria-labelledby="stats-title">
          <div className="card-heading">
            <h3 id="stats-title">Project inventory</h3>
            <span>4 metrics</span>
          </div>
          <div className="project-overview-counts">
            {stats.map((stat) => (
              <article className="stat-tile" key={stat.label}>
                <span className="stat-value">{stat.value}</span>
                <span className="stat-label">{stat.label}</span>
              </article>
            ))}
          </div>
        </section>
      </div>
    </section>
  );
}
