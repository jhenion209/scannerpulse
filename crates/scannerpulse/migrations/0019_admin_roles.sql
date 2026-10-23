-- Admin roles (build step 7): a super admin may change everything; an
-- associate only what a super admin turned on for that person. Every admin
-- that exists now becomes a super admin; one added later is an associate
-- unless made a super admin, so a first sign-in through the OIDC provider
-- arrives as an associate with every function off.
ALTER TABLE admins
    ADD COLUMN role text NOT NULL DEFAULT 'super'
        CONSTRAINT admins_role CHECK (role IN ('super', 'associate')),
    ADD COLUMN functions text[] NOT NULL DEFAULT '{}';
ALTER TABLE admins ALTER COLUMN role SET DEFAULT 'associate';
