import { HashRouter, Route, Routes } from 'react-router-dom';
import { Layout } from './components/Layout';
import { Apps } from './pages/Apps';
import { Components } from './pages/Components';
import { Dashboard } from './pages/Dashboard';
import { Health } from './pages/Health';
import { Sessions } from './pages/Sessions';
import { Settings } from './pages/Settings';

export function App() {
  return (
    <HashRouter>
      <Layout>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/components" element={<Components />} />
          <Route path="/apps" element={<Apps />} />
          <Route path="/sessions" element={<Sessions />} />
          <Route path="/health" element={<Health />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </Layout>
    </HashRouter>
  );
}
