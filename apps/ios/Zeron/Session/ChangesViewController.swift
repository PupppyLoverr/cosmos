import UIKit

final class ChangesAccessoryPill: UIControl {
    var onTap: (() -> Void)?

    private let glass = Glass.surface(interactive: true)
    private let title = UILabel()
    private let additions = UILabel()
    private let deletions = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isAccessibilityElement = true
        accessibilityIdentifier = "session-changes"
        accessibilityTraits = .button
        isHidden = true

        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)

        title.text = "Changes"
        title.textColor = Palette.text
        title.font = Fonts.ui(.sansMedium, 12.5)
        additions.textColor = Palette.success
        deletions.textColor = Palette.danger
        for label in [additions, deletions] {
            label.font = Fonts.ui(.mono, 12.5)
        }

        let content = UIStackView(arrangedSubviews: [title, additions, deletions])
        content.axis = .horizontal
        content.alignment = .center
        content.spacing = 6
        content.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(content)
        NSLayoutConstraint.activate([
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            content.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 12),
            content.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -12),
            content.topAnchor.constraint(equalTo: glass.contentView.topAnchor, constant: 8),
            content.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor, constant: -8),
            heightAnchor.constraint(equalToConstant: 34),
        ])
        addAction(UIAction { [weak self] _ in self?.onTap?() }, for: .touchUpInside)
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ changes: SessionChanges) {
        additions.text = "+\(changes.additions)"
        deletions.text = "−\(changes.deletions)"
        isHidden = changes.files.isEmpty
        accessibilityLabel = "Changes, \(changes.files.count) files, +\(changes.additions), −\(changes.deletions)"
    }
}

final class ModelAccessoryPill: UIControl {
    var onTap: (() -> Void)?

    private let glass = Glass.surface(interactive: true)
    private let mark = UIImageView()
    private let label = UILabel()
    private let chevron = UIImageView(image: UIImage(systemName: "chevron.down"))

    override init(frame: CGRect) {
        super.init(frame: frame)
        isAccessibilityElement = true
        accessibilityIdentifier = "session-model"
        accessibilityTraits = .button

        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        mark.contentMode = .scaleAspectFit
        mark.translatesAutoresizingMaskIntoConstraints = false
        mark.isAccessibilityElement = false
        label.font = Fonts.ui(.sansMedium, 13)
        label.textColor = Palette.text
        label.lineBreakMode = .byTruncatingTail
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        chevron.tintColor = Palette.tertiary
        chevron.contentMode = .scaleAspectFit
        chevron.translatesAutoresizingMaskIntoConstraints = false
        chevron.isAccessibilityElement = false
        let content = UIStackView(arrangedSubviews: [mark, label, chevron])
        content.axis = .horizontal
        content.alignment = .center
        content.spacing = 6
        content.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(content)
        NSLayoutConstraint.activate([
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            content.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 10),
            content.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -10),
            content.topAnchor.constraint(equalTo: glass.contentView.topAnchor, constant: 8),
            content.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor, constant: -8),
            mark.widthAnchor.constraint(equalToConstant: 13),
            mark.heightAnchor.constraint(equalToConstant: 13),
            chevron.widthAnchor.constraint(equalToConstant: 9),
            chevron.heightAnchor.constraint(equalToConstant: 9),
            heightAnchor.constraint(equalToConstant: 34),
            widthAnchor.constraint(lessThanOrEqualToConstant: 190),
        ])
        addAction(UIAction { [weak self] _ in self?.onTap?() }, for: .touchUpInside)
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(harness: String, label text: String) {
        mark.image = BrandMarks.image(for: harness, side: 13)
        mark.tintColor = Palette.secondary
        label.text = text
        accessibilityLabel = "Model"
        accessibilityValue = text
    }
}

final class ChangesViewController: UIViewController {
    private let changes: SessionChanges

    init(changes: SessionChanges) {
        self.changes = changes
        super.init(nibName: nil, bundle: nil)
        title = "Changes"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background

        let title = UILabel()
        title.text = "Changes"
        title.textColor = Palette.text
        title.font = Fonts.ui(.sansSemibold, 23)
        let summary = UILabel()
        summary.text = "\(changes.files.count) files · +\(changes.additions) −\(changes.deletions)"
        summary.textColor = Palette.secondary
        summary.font = Fonts.ui(.mono, 12.5)

        let rows = UIStackView()
        rows.axis = .vertical
        rows.alignment = .fill
        rows.spacing = 0
        rows.translatesAutoresizingMaskIntoConstraints = false
        for (index, file) in changes.files.enumerated() {
            rows.addArrangedSubview(ChangeFileRow(file: file, index: index))
        }
        let scroll = UIScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.alwaysBounceVertical = false
        scroll.addSubview(rows)
        title.translatesAutoresizingMaskIntoConstraints = false
        summary.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(title)
        view.addSubview(summary)
        view.addSubview(scroll)
        NSLayoutConstraint.activate([
            title.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 22),
            title.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),
            title.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
            summary.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 6),
            summary.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            summary.trailingAnchor.constraint(equalTo: title.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: summary.bottomAnchor, constant: 16),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 12),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -12),
            scroll.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -8),
            rows.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            rows.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            rows.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            rows.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor),
        ])
    }
}

private final class ChangeFileRow: UIView {
    init(file: ChangedFile, index: Int) {
        super.init(frame: .zero)
        accessibilityIdentifier = "change-file-row-\(index)"
        isAccessibilityElement = true
        accessibilityLabel = "\(file.path), +\(file.additions), −\(file.deletions)"

        let icon = UIImageView(image: UIImage(systemName: Self.icon(for: file.path)))
        icon.tintColor = Palette.secondary
        icon.contentMode = .scaleAspectFit
        icon.translatesAutoresizingMaskIntoConstraints = false
        icon.isAccessibilityElement = false
        let path = UILabel()
        path.text = file.path
        path.textColor = Palette.text
        path.font = Fonts.ui(.mono, 12.5)
        path.lineBreakMode = .byTruncatingHead
        path.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let additions = UILabel()
        additions.text = "+\(file.additions)"
        additions.textColor = Palette.success
        additions.font = Fonts.ui(.mono, 12.5)
        let deletions = UILabel()
        deletions.text = "−\(file.deletions)"
        deletions.textColor = Palette.danger
        deletions.font = Fonts.ui(.mono, 12.5)
        let counts = UIStackView(arrangedSubviews: [additions, deletions])
        counts.axis = .horizontal
        counts.alignment = .center
        counts.spacing = 8
        counts.setContentCompressionResistancePriority(.required, for: .horizontal)
        let content = UIStackView(arrangedSubviews: [icon, path, counts])
        content.axis = .horizontal
        content.alignment = .center
        content.spacing = 9
        content.translatesAutoresizingMaskIntoConstraints = false
        addSubview(content)
        let separator = UIView()
        separator.translatesAutoresizingMaskIntoConstraints = false
        separator.backgroundColor = Palette.hairline
        addSubview(separator)
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            content.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            content.topAnchor.constraint(equalTo: topAnchor, constant: 9),
            content.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -9),
            icon.widthAnchor.constraint(equalToConstant: 16),
            icon.heightAnchor.constraint(equalToConstant: 18),
            heightAnchor.constraint(greaterThanOrEqualToConstant: 42),
            separator.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            separator.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            separator.bottomAnchor.constraint(equalTo: bottomAnchor),
            separator.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    private static func icon(for path: String) -> String {
        switch URL(fileURLWithPath: path).pathExtension.lowercased() {
        case "swift": "swift"
        case "rs", "c", "cc", "cpp", "h", "js", "jsx", "ts", "tsx": "chevron.left.forwardslash.chevron.right"
        case "md", "markdown": "doc.richtext"
        default: "doc.text"
        }
    }
}
