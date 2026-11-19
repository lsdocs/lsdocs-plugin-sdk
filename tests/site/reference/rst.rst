:author: The lsdocs authors
:summary: A page written in reStructuredText.

.. _rst-page:

reStructuredText
================

This page is parsed by ``parser-rst``. It links to the
:ref:`configuration guide <config>` by label, and to `its own
section`_ by title.

Its own section
---------------

.. note::

   Admonitions, footnotes [#]_ and math such as :math:`a^2 + b^2` have
   core node kinds.

.. [#] Numbered automatically.

.. code:: rust

   fn main() {
       println!("hello");
   }

term
   A definition.

.. figure:: ../img/logo.svg
   :alt: The lsdocs logo
   :name: logo

   The logo, as a figure.

=====  ======
Input  Output
=====  ======
rst    html
=====  ======

.. mermaid::

   graph LR; rst --> html
