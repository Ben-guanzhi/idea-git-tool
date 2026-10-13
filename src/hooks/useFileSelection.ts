// The selected working-tree file: split verbatim from App.tsx:52. Lives in its
// own hook so the commit-flow keyboard effect keeps its original position
// before the file-diff loading effect.

import { useState } from "react";

export function useFileSelection() {
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  return { selectedFile, setSelectedFile };
}
