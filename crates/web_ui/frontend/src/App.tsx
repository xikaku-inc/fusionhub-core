import { useEffect } from 'react';
import { HashRouter, Routes, Route, Navigate } from 'react-router-dom';
import Layout from './components/Layout/Layout';
import Dashboard from './components/Dashboard/Dashboard';
import LicenseView from './components/License/LicenseView';
import NodeEditor from './components/NodeEditor/NodeEditor';
import ExtensionLoader from './components/Extensions/ExtensionLoader';
import { useSSE } from './hooks/useSSE';
import { useAppStore } from './stores/appStore';
import { useActiveExtensions } from './hooks/useActiveExtensions';

export default function App() {
  useSSE();
  const fetchNodeTypes = useAppStore((s) => s.fetchNodeTypes);
  const fetchUiExtensions = useAppStore((s) => s.fetchUiExtensions);
  const activeExtensions = useActiveExtensions();
  useEffect(() => { fetchNodeTypes(); fetchUiExtensions(); }, [fetchNodeTypes, fetchUiExtensions]);
  return (
    <HashRouter>
      <Layout>
        <Routes>
          <Route path="/" element={<Navigate to="/dashboard" replace />} />
          <Route path="/dashboard" element={<Dashboard />} />
          <Route path="/license" element={<LicenseView />} />
          <Route path="/node-editor" element={<NodeEditor />} />
          {activeExtensions.map((ext) => (
            <Route key={ext.id} path={ext.route} element={<ExtensionLoader key={ext.id} extension={ext} />} />
          ))}
        </Routes>
      </Layout>
    </HashRouter>
  );
}
