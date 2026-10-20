-- The root helper also carries out the steps of a conversion to high
-- availability that an admin approves in the admin page's guide. Additive:
-- the kinds the older version asks for stay allowed.
ALTER TABLE agent_requests DROP CONSTRAINT agent_requests_kind_check;
ALTER TABLE agent_requests ADD CONSTRAINT agent_requests_kind_check CHECK (kind IN (
    'update', 'switchover',
    'archive_check', 'archive_switch', 'db_move', 'db_switch', 'db_retire', 'second'
));
