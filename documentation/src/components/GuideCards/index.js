import React from 'react';
import Link from '@docusaurus/Link';
import styles from './styles.module.css';

export default function GuideCards({guides}) {
  return (
    <div className={styles.grid}>
      {guides.map((guide) => (
        <Link key={guide.href} to={guide.href} className={styles.card}>
          <h2 className={styles.title}>{guide.title}</h2>
          <p className={styles.description}>{guide.description}</p>
          <div className={styles.tags}>
            {guide.tags.map((tag) => <span key={tag} className={styles.tag}>{tag}</span>)}
          </div>
        </Link>
      ))}
    </div>
  );
}
