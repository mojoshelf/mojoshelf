-- Packages are now called tins (previously books).
ALTER TABLE books RENAME TO tins;
ALTER TABLE versions RENAME COLUMN book_id TO tin_id;
ALTER TABLE dependencies RENAME COLUMN depends_on_book_id TO depends_on_tin_id;
