-- Core Organizations & Tenancy
CREATE TABLE IF NOT EXISTS organizations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    slug VARCHAR(64) UNIQUE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(255),
    full_name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Projects
CREATE TABLE IF NOT EXISTS projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    slug VARCHAR(64) NOT NULL,
    description TEXT,
    git_remote_url TEXT,
    visibility VARCHAR(32) NOT NULL DEFAULT 'private', -- 'public', 'private', 'internal'
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(org_id, slug)
);

-- Revisions / Commits
CREATE TABLE IF NOT EXISTS design_revisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    git_commit_hash VARCHAR(40) NOT NULL,
    git_branch VARCHAR(128) NOT NULL,
    author_name VARCHAR(255) NOT NULL,
    commit_message TEXT NOT NULL,
    render_status VARCHAR(32) NOT NULL DEFAULT 'pending', -- 'pending', 'processing', 'completed', 'failed'
    schematic_assets JSONB DEFAULT '{}', -- maps sheet names to S3 keys of SVGs
    pcb_layer_assets JSONB DEFAULT '{}',  -- maps layer names (F.Cu, B.Cu) to S3 keys
    glb_3d_asset_url TEXT,
    drc_summary JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(project_id, git_commit_hash)
);

-- Review Comments (Redlining & Visual Pins)
CREATE TABLE IF NOT EXISTS review_threads (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    revision_id UUID NOT NULL REFERENCES design_revisions(id) ON DELETE CASCADE,
    file_type VARCHAR(16) NOT NULL, -- 'pcb' or 'schematic'
    sheet_or_layer VARCHAR(64) NOT NULL,
    coord_x DOUBLE PRECISION NOT NULL, -- Real board coordinate in mm
    coord_y DOUBLE PRECISION NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'open', -- 'open', 'resolved', 'ignored'
    created_by UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    resolved_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS review_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    thread_id UUID NOT NULL REFERENCES review_threads(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id),
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Centralized Component Library (Enterprise Tier)
CREATE TABLE IF NOT EXISTS parts_library (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    mpn VARCHAR(255) NOT NULL,          -- Manufacturer Part Number
    manufacturer VARCHAR(255) NOT NULL,
    description TEXT,
    value VARCHAR(64),                  -- e.g. "10k", "100nF"
    footprint_name VARCHAR(255) NOT NULL,
    symbol_name VARCHAR(255) NOT NULL,
    datasheet_url TEXT,
    lifecycle_status VARCHAR(32) NOT NULL DEFAULT 'active', -- 'active', 'nrnd', 'obsolete'
    stock_status_cache JSONB DEFAULT '{}', -- DigiKey/Mouser cached prices & inventory
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_parts_mpn ON parts_library(org_id, mpn);
