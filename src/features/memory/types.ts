// Frontend types for memory entries and search results.

export interface MemoryIndexEntry {
  name: string;
  description: string;
  tags: string[];
  memoryType: string;
  fileName: string;
}

export interface MemorySearchResult {
  name: string;
  description: string;
  memoryType: string;
  snippet: string;
  score: number;
}

export interface ParsedMemory {
  name: string;
  description: string;
  type: string;
  tags: string[];
  links: string[];
  body: string;
}
