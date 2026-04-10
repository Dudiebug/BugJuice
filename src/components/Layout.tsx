import type { ReactNode } from 'react';
import { Sidebar } from './Sidebar';

export function Layout({ children }: { children: ReactNode }) {
  return (
    <div className="app-root">
      <Sidebar />
      <main className="content">{children}</main>
    </div>
  );
}
