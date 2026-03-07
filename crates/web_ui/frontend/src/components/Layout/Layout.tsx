import { useState, useMemo, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useAppStore } from '../../stores/appStore';
import { useActiveExtensions } from '../../hooks/useActiveExtensions';

interface LayoutProps {
  children: ReactNode;
}

const coreNavItems = [
  { path: '/dashboard', label: 'Dashboard' },
  { path: '/node-editor', label: 'Node Editor' },
  { path: '/logs', label: 'Logs' },
  { path: '/license', label: 'License' },
];

export default function Layout({ children }: LayoutProps) {
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const location = useLocation();
  const navigate = useNavigate();
  const sseConnected = useAppStore((s) => s.sseConnected);
  const activeExtensions = useActiveExtensions();

  const extensionSections = useMemo(() => {
    const sections: Record<string, { path: string; label: string }[]> = {};
    for (const ext of activeExtensions) {
      const section = ext.navSection || 'Extensions';
      if (!sections[section]) sections[section] = [];
      sections[section].push({ path: ext.route, label: ext.displayName });
    }
    return sections;
  }, [activeExtensions]);

  return (
    <>
      <header className="app-bar">
        <button className="nav-toggle" onClick={() => setSidebarOpen(!sidebarOpen)}>
          &#9776;
        </button>
        <span className="app-title">FusionHub Control</span>
        <span className="spacer" />
        <span className={`badge ${sseConnected ? 'connected' : 'disconnected'}`}>
          {sseConnected ? 'Connected' : 'Disconnected'}
        </span>
      </header>

      <nav className={`sidebar ${sidebarOpen ? 'open' : ''}`}>
        <div className="nav-section">
          <div className="nav-title">Pages</div>
          {coreNavItems.map((item) => (
            <a
              key={item.path}
              className={`nav-item ${location.pathname === item.path ? 'active' : ''}`}
              onClick={() => navigate(item.path)}
            >
              {item.label}
            </a>
          ))}
        </div>
        {Object.entries(extensionSections).map(([section, items]) => (
          <div className="nav-section" key={section}>
            <div className="nav-title">{section}</div>
            {items.map((item) => (
              <a
                key={item.path}
                className={`nav-item ${location.pathname === item.path ? 'active' : ''}`}
                onClick={() => navigate(item.path)}
              >
                {item.label}
              </a>
            ))}
          </div>
        ))}
      </nav>

      <main className={`content ${sidebarOpen ? 'sidebar-open' : ''}`}>
        {children}
      </main>
    </>
  );
}
