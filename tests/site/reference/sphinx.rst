.. _sphinx-page:

Sphinx add-ons
==============

This page uses the directives and roles of ``sphinx-addons``. They work
in Markdown too: see :doc:`../guide/myst`.

.. index::
   single: plugins; sphinx-addons
   pair: Sphinx; directives

.. versionadded:: 0.1
   Pages can use Sphinx's directives.

.. code-block:: toml
   :caption: A plugin entry
   :name: plugin-entry
   :emphasize-lines: 3

   [[plugins]]
   name = "sphinx"
   builtin = "sphinx-addons"

:numref:`plugin-entry` shows the entry. Press :kbd:`Ctrl+C` to stop
:command:`lsdocs serve`, pick :menuselection:`File --> &Save`, and read
the :abbr:`TOC (table of contents)`. Everything indexed is in the
:ref:`genindex`.

.. rubric:: Terms
   :heading-level: 2

.. glossary::

   Plugin
      A program lsdocs talks to.

.. toctree::
   :caption: More pages
   :maxdepth: 2

   ../guide/myst
   cli
