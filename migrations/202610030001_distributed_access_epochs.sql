CREATE FUNCTION notify_document_access_epoch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.access_epoch IS DISTINCT FROM OLD.access_epoch THEN
        PERFORM pg_notify(
            'pxel_document_access_epoch',
            NEW.document_id::text || ':' || NEW.access_epoch::text
        );
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER documents_access_epoch_notify
    AFTER UPDATE OF access_epoch ON documents
    FOR EACH ROW EXECUTE FUNCTION notify_document_access_epoch();
